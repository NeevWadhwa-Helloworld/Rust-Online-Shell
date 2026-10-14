#[allow(unused_imports)]
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};

#[cfg(unix)]
use std::os::fd::OwnedFd;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

enum RedirectMode {
    Overwrite,
    Append,
}

struct Redirection {
    path: String,
    mode: RedirectMode,
}

struct Job {
    id: usize,
    #[allow(dead_code)]
    pid: u32,
    command: String,
    child: Child,
}

fn reap_jobs(jobs: &mut Vec<Job>) {
    let total_jobs = jobs.len();
    let mut remaining_jobs = Vec::new();

    for (idx, mut job) in std::mem::take(jobs).into_iter().enumerate() {
        let marker = if idx == total_jobs - 1 {
            '+'
        } else if idx == total_jobs - 2 {
            '-'
        } else {
            ' '
        };

        let is_finished = match job.child.try_wait() {
            Ok(Some(_status)) => true,
            _ => false,
        };

        if is_finished {
            let status_padded = format!("{:24}", "Done");
            let display_cmd = job.command.trim_end_matches('&').trim_end();
            println!("[{}]{} {}  {}", job.id, marker, status_padded, display_cmd);
        } else {
            remaining_jobs.push(job);
        }
    }

    *jobs = remaining_jobs;
}

fn is_builtin(cmd: &str) -> bool {
    matches!(
        cmd,
        "echo"
            | "pwd"
            | "cd"
            | "type"
            | "complete"
            | "jobs"
            | "exit"
            | "history"
            | "declare"
            | "ls"
            | "mkdir"
            | "cat"
            | "rm"
            | "rmdir"
            | "touch"
            | "cp"
            | "mv"
            | "head"
            | "tail"
            | "grep"
            | "wc"
            | "clear"
            | "export"
            | "unset"
            | "env"
            | "printenv"
            | "alias"
            | "unalias"
            | "which"
            | "help"
    )
}

fn save_history_to_file(path: &str, history: &[String]) {
    if let Ok(mut file) = std::fs::File::create(path) {
        for entry in history {
            let _ = writeln!(file, "{}", entry);
        }
    }
}

fn parse_redirections(
    tokens: &[String],
) -> (Vec<String>, Option<Redirection>, Option<Redirection>, Option<String>) {
    let mut stdout_redir = None;
    let mut stderr_redir = None;
    let mut stdin_redir = None;
    let mut clean_tokens = Vec::new();

    let mut i = 0;
    while i < tokens.len() {
        if (tokens[i] == ">>" || tokens[i] == "1>>") && i + 1 < tokens.len() {
            stdout_redir = Some(Redirection {
                path: tokens[i + 1].clone(),
                mode: RedirectMode::Append,
            });
            i += 2;
        } else if (tokens[i] == ">" || tokens[i] == "1>") && i + 1 < tokens.len() {
            stdout_redir = Some(Redirection {
                path: tokens[i + 1].clone(),
                mode: RedirectMode::Overwrite,
            });
            i += 2;
        } else if tokens[i] == "2>>" && i + 1 < tokens.len() {
            stderr_redir = Some(Redirection {
                path: tokens[i + 1].clone(),
                mode: RedirectMode::Append,
            });
            i += 2;
        } else if tokens[i] == "2>" && i + 1 < tokens.len() {
            stderr_redir = Some(Redirection {
                path: tokens[i + 1].clone(),
                mode: RedirectMode::Overwrite,
            });
            i += 2;
        } else if tokens[i] == "<" && i + 1 < tokens.len() {
            stdin_redir = Some(tokens[i + 1].clone());
            i += 2;
        } else {
            clean_tokens.push(tokens[i].clone());
            i += 1;
        }
    }

    (clean_tokens, stdout_redir, stderr_redir, stdin_redir)
}

fn expand_var_at(
    chars: &[char],
    i: &mut usize,
    shell_vars: &BTreeMap<String, String>,
    last_exit_code: i32,
) -> String {
    *i += 1; // skip '$'
    if *i >= chars.len() {
        return "$".to_string();
    }
    if chars[*i] == '?' {
        *i += 1;
        return last_exit_code.to_string();
    }
    if chars[*i] == '$' {
        *i += 1;
        return std::process::id().to_string();
    }
    if chars[*i] == '{' {
        *i += 1;
        let start = *i;
        while *i < chars.len() && chars[*i] != '}' {
            *i += 1;
        }
        let var_name: String = chars[start..*i].iter().collect();
        if *i < chars.len() {
            *i += 1; // skip '}'
        }
        if let Some(val) = shell_vars.get(&var_name) {
            return val.clone();
        } else if let Ok(val) = std::env::var(&var_name) {
            return val;
        }
        return String::new();
    }
    let start = *i;
    while *i < chars.len() && (chars[*i].is_ascii_alphanumeric() || chars[*i] == '_') {
        *i += 1;
    }
    let var_name: String = chars[start..*i].iter().collect();
    if var_name.is_empty() {
        "$".to_string()
    } else if let Some(val) = shell_vars.get(&var_name) {
        val.clone()
    } else if let Ok(val) = std::env::var(&var_name) {
        val
    } else {
        String::new()
    }
}

fn is_valid_identifier(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

const MAX_STORAGE_BYTES: u64 = 50 * 1024 * 1024; // 50MB quota per user

fn get_dir_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    total += get_dir_size(&entry.path());
                } else if let Ok(meta) = entry.metadata() {
                    total += meta.len();
                }
            }
        }
    }
    total
}

fn check_quota_available() -> bool {
    if let Ok(home) = std::env::var("HOME") {
        let home_path = Path::new(&home);
        if home_path.exists() {
            return get_dir_size(home_path) < MAX_STORAGE_BYTES;
        }
    }
    true
}

fn is_path_safe(target_path: &str) -> bool {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return true,
    };
    let home_path = Path::new(&home);
    let canon_home = match home_path.canonicalize() {
        Ok(c) => c,
        Err(_) => home_path.to_path_buf(),
    };

    let base = std::env::current_dir().unwrap_or_else(|_| canon_home.clone());
    let candidate = if Path::new(target_path).is_absolute() {
        Path::new(target_path).to_path_buf()
    } else {
        base.join(target_path)
    };

    // If candidate exists, its canonical form MUST start with canon_home
    if let Ok(canon) = candidate.canonicalize() {
        return canon.starts_with(&canon_home);
    }

    // If candidate does NOT exist yet, find its closest existing ancestor
    let mut cur = candidate.as_path();
    while !cur.exists() {
        match cur.parent() {
            Some(p) => cur = p,
            None => return false,
        }
    }

    if let Ok(canon_cur) = cur.canonicalize() {
        if !canon_cur.starts_with(&canon_home) {
            return false;
        }
    } else {
        return false;
    }

    // Ensure components do not escape above the existing ancestor
    let mut depth: isize = 0;
    if let Ok(rel) = candidate.strip_prefix(cur) {
        for comp in rel.components() {
            match comp {
                std::path::Component::ParentDir => {
                    depth -= 1;
                    if depth < 0 {
                        return false;
                    }
                }
                std::path::Component::Normal(_) => {
                    depth += 1;
                }
                _ => {}
            }
        }
    }

    true
}

fn copy_dir_all(src: &Path, dst: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let target_dst = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_all(&entry.path(), &target_dst)?;
        } else {
            std::fs::copy(entry.path(), &target_dst)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn execute_builtin(
    cmd: &str,
    args: &[String],
    completion_specs: &mut BTreeMap<String, String>,
    jobs: &mut Vec<Job>,
    history: &mut Vec<String>,
    history_appended_index: &mut usize,
    shell_vars: &mut BTreeMap<String, String>,
    exported_vars: &mut BTreeSet<String>,
    aliases: &mut BTreeMap<String, String>,
    in_stream: &mut dyn Read,
    out: &mut dyn Write,
    err_out: &mut dyn Write,
) -> (bool, i32) {
    if cmd == "echo" {
        let _ = writeln!(out, "{}", args.join(" "));
        (false, 0)
    } else if cmd == "pwd" {
        if let Ok(current_dir) = std::env::current_dir() {
            let _ = writeln!(out, "{}", current_dir.display());
            (false, 0)
        } else {
            let _ = writeln!(err_out, "pwd: error getting current directory");
            (false, 1)
        }
    } else if cmd == "cd" {
        let home_dir = std::env::var("HOME").ok();
        let target = match args.first().map(|s| s.as_str()) {
            Some("~") | None => home_dir.clone(),
            Some(path) => Some(path.to_string()),
        };
        if let Some(target_path) = target {
            let next_path = Path::new(&target_path);
            let candidate = if next_path.is_absolute() {
                next_path.to_path_buf()
            } else {
                std::env::current_dir().unwrap_or_default().join(next_path)
            };
            if !candidate.exists() {
                let _ = writeln!(err_out, "cd: {}: No such file or directory", target_path);
                (false, 1)
            } else {
                let is_allowed = match (&home_dir, candidate.canonicalize()) {
                    (Some(home), Ok(canon)) => {
                        let canon_home = Path::new(home)
                            .canonicalize()
                            .unwrap_or_else(|_| Path::new(home).to_path_buf());
                        canon.starts_with(&canon_home)
                    }
                    _ => true,
                };
                if !is_allowed {
                    let _ = writeln!(err_out, "cd: {}: Permission denied (sandboxed)", target_path);
                    (false, 1)
                } else if let Err(e) = std::env::set_current_dir(&candidate) {
                    let _ = writeln!(err_out, "cd: {}: {}", target_path, e);
                    (false, 1)
                } else {
                    (false, 0)
                }
            }
        } else {
            (false, 0)
        }
    } else if cmd == "type" {
        let mut exit_code = 0;
        if args.is_empty() {
            exit_code = 1;
        } else {
            for cmd_arg in args {
                if is_builtin(cmd_arg) {
                    let _ = writeln!(out, "{} is a shell builtin", cmd_arg);
                } else if aliases.contains_key(cmd_arg.as_str()) {
                    let _ = writeln!(out, "{} is aliased to '{}'", cmd_arg, aliases[cmd_arg.as_str()]);
                } else if let Some(path) = get_executable_path(cmd_arg) {
                    let _ = writeln!(out, "{} is {}", cmd_arg, path.display());
                } else {
                    let _ = writeln!(out, "{}: not found", cmd_arg);
                    exit_code = 1;
                }
            }
        }
        (false, exit_code)
    } else if cmd == "complete" {
        if args.first().map(|s| s.as_str()) == Some("-C") {
            if args.len() >= 3 {
                let script_path = &args[1];
                let target_cmd = &args[2];
                completion_specs.insert(target_cmd.clone(), script_path.clone());
            }
            (false, 0)
        } else if args.first().map(|s| s.as_str()) == Some("-p") {
            if let Some(target_cmd) = args.get(1) {
                if let Some(script_path) = completion_specs.get(target_cmd) {
                    let _ = writeln!(out, "complete -C '{}' {}", script_path, target_cmd);
                    (false, 0)
                } else {
                    let _ = writeln!(err_out, "complete: {}: no completion specification", target_cmd);
                    (false, 1)
                }
            } else {
                for (cmd_name, script_path) in completion_specs.iter() {
                    let _ = writeln!(out, "complete -C '{}' {}", script_path, cmd_name);
                }
                (false, 0)
            }
        } else if args.first().map(|s| s.as_str()) == Some("-r") {
            if let Some(target_cmd) = args.get(1) {
                completion_specs.remove(target_cmd);
            } else {
                completion_specs.clear();
            }
            (false, 0)
        } else {
            (false, 0)
        }
    } else if cmd == "jobs" {
        let total_jobs = jobs.len();
        let mut remaining_jobs = Vec::new();

        for (idx, mut job) in std::mem::take(jobs).into_iter().enumerate() {
            let marker = if idx == total_jobs - 1 {
                '+'
            } else if idx == total_jobs - 2 {
                '-'
            } else {
                ' '
            };

            let is_finished = match job.child.try_wait() {
                Ok(Some(_status)) => true,
                _ => false,
            };

            if is_finished {
                let status_padded = format!("{:24}", "Done");
                let display_cmd = job.command.trim_end_matches('&').trim_end();
                let _ = writeln!(out, "[{}]{} {}  {}", job.id, marker, status_padded, display_cmd);
            } else {
                let status_padded = format!("{:24}", "Running");
                let _ = writeln!(out, "[{}]{} {}  {}", job.id, marker, status_padded, job.command);
                remaining_jobs.push(job);
            }
        }
        *jobs = remaining_jobs;
        (false, 0)
    } else if cmd == "history" {
        if args.first().map(|s| s.as_str()) == Some("-r") {
            if let Some(path) = args.get(1) {
                if let Ok(content) = std::fs::read_to_string(path) {
                    for line in content.lines() {
                        if !line.is_empty() {
                            history.push(line.to_string());
                        }
                    }
                }
            }
        } else if args.first().map(|s| s.as_str()) == Some("-w") {
            if let Some(path) = args.get(1) {
                save_history_to_file(path, history);
            }
        } else if args.first().map(|s| s.as_str()) == Some("-a") {
            if let Some(path) = args.get(1) {
                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                    for entry in history.iter().skip(*history_appended_index) {
                        let _ = writeln!(file, "{}", entry);
                    }
                    *history_appended_index = history.len();
                }
            }
        } else {
            let count = args.first().and_then(|s| s.parse::<usize>().ok()).unwrap_or(history.len());
            let start_idx = history.len().saturating_sub(count);
            for (i, entry) in history.iter().enumerate().skip(start_idx) {
                let _ = writeln!(out, "{:>5}  {}", i + 1, entry);
            }
        }
        (false, 0)
    } else if cmd == "declare" {
        if args.first().map(|s| s.as_str()) == Some("-p") {
            if let Some(var_name) = args.get(1) {
                if let Some(val) = shell_vars.get(var_name) {
                    let _ = writeln!(out, "declare -- {}=\"{}\"", var_name, val);
                    (false, 0)
                } else {
                    let _ = writeln!(err_out, "declare: {}: not found", var_name);
                    (false, 1)
                }
            } else {
                for (k, v) in shell_vars.iter() {
                    let _ = writeln!(out, "declare -- {}=\"{}\"", k, v);
                }
                (false, 0)
            }
        } else if let Some(arg) = args.first() {
            if let Some((k, v)) = arg.split_once('=') {
                if is_valid_identifier(k) {
                    shell_vars.insert(k.to_string(), v.to_string());
                    (false, 0)
                } else {
                    let _ = writeln!(err_out, "declare: `{}': not a valid identifier", arg);
                    (false, 1)
                }
            } else if is_valid_identifier(arg) {
                shell_vars.entry(arg.to_string()).or_default();
                (false, 0)
            } else {
                let _ = writeln!(err_out, "declare: `{}': not a valid identifier", arg);
                (false, 1)
            }
        } else {
            for (k, v) in shell_vars.iter() {
                let _ = writeln!(out, "declare -- {}=\"{}\"", k, v);
            }
            (false, 0)
        }
    } else if cmd == "export" {
        let mut exit_code = 0;
        for arg in args {
            if let Some((k, v)) = arg.split_once('=') {
                if is_valid_identifier(k) {
                    shell_vars.insert(k.to_string(), v.to_string());
                    exported_vars.insert(k.to_string());
                    std::env::set_var(k, v);
                } else {
                    let _ = writeln!(err_out, "export: `{}': not a valid identifier", arg);
                    exit_code = 1;
                }
            } else if is_valid_identifier(arg) {
                exported_vars.insert(arg.clone());
                if let Some(val) = shell_vars.get(arg) {
                    std::env::set_var(arg, val);
                }
            } else {
                let _ = writeln!(err_out, "export: `{}': not a valid identifier", arg);
                exit_code = 1;
            }
        }
        if args.is_empty() {
            for (k, v) in std::env::vars() {
                let _ = writeln!(out, "declare -x {}=\"{}\"", k, v);
            }
        }
        (false, exit_code)
    } else if cmd == "unset" {
        for arg in args {
            shell_vars.remove(arg.as_str());
            exported_vars.remove(arg.as_str());
            std::env::remove_var(arg);
        }
        (false, 0)
    } else if cmd == "env" || cmd == "printenv" {
        for (k, v) in std::env::vars() {
            let _ = writeln!(out, "{}={}", k, v);
        }
        (false, 0)
    } else if cmd == "alias" {
        if args.is_empty() {
            for (k, v) in aliases.iter() {
                let _ = writeln!(out, "alias {}='{}'", k, v);
            }
            (false, 0)
        } else {
            let mut exit_code = 0;
            for arg in args {
                if let Some((k, v)) = arg.split_once('=') {
                    let val = v.trim_matches('\'').trim_matches('"').to_string();
                    aliases.insert(k.to_string(), val);
                } else if let Some(val) = aliases.get(arg.as_str()) {
                    let _ = writeln!(out, "alias {}='{}'", arg, val);
                } else {
                    let _ = writeln!(err_out, "alias: {}: not found", arg);
                    exit_code = 1;
                }
            }
            (false, exit_code)
        }
    } else if cmd == "unalias" {
        for arg in args {
            aliases.remove(arg.as_str());
        }
        (false, 0)
    } else if cmd == "which" {
        let mut exit_code = 0;
        if args.is_empty() {
            exit_code = 1;
        } else {
            for arg in args {
                if is_builtin(arg) {
                    let _ = writeln!(out, "{}: shell builtin", arg);
                } else if let Some(path) = get_executable_path(arg) {
                    let _ = writeln!(out, "{}", path.display());
                } else {
                    let _ = writeln!(err_out, "which: no {} in PATH", arg);
                    exit_code = 1;
                }
            }
        }
        (false, exit_code)
    } else if cmd == "help" {
        let _ = writeln!(out, "Rust Online Shell — Built-in Commands");
        let _ = writeln!(out, "");
        let _ = writeln!(out, "  File Operations:");
        let _ = writeln!(out, "    ls [-a|-l] [dir]      list directory contents (hidden files with -a)");
        let _ = writeln!(out, "    cd [dir]              change directory (sandboxed to HOME)");
        let _ = writeln!(out, "    pwd                   print working directory");
        let _ = writeln!(out, "    mkdir [-p] <dir>      create directory");
        let _ = writeln!(out, "    rmdir <dir>           remove empty directory");
        let _ = writeln!(out, "    rm [-r|-rf|-f] <path> remove file or directory");
        let _ = writeln!(out, "    cp [-r] <src> <dst>   copy file or directory");
        let _ = writeln!(out, "    mv <src> <dst>        move/rename file or directory");
        let _ = writeln!(out, "    touch <file>          create or update timestamp (non-destructive)");
        let _ = writeln!(out, "    cat [file...]         print file contents or stdin");
        let _ = writeln!(out, "    head [-n N|-N] [file] print first N lines (default 10)");
        let _ = writeln!(out, "    tail [-n N|-N] [file] print last N lines (default 10)");
        let _ = writeln!(out, "    grep [-i|-v|-n] <pat> search pattern in files or stdin");
        let _ = writeln!(out, "    wc [-l|-w|-c] [file]  count lines/words/bytes");
        let _ = writeln!(out, "");
        let _ = writeln!(out, "  Variables & Environment:");
        let _ = writeln!(out, "    declare [var=val]     set or inspect shell variables");
        let _ = writeln!(out, "    export [var[=val]]    export variable to child processes");
        let _ = writeln!(out, "    unset <var>           remove variable");
        let _ = writeln!(out, "    env / printenv        print all environment variables");
        let _ = writeln!(out, "    alias [name=val]      set or list aliases");
        let _ = writeln!(out, "    unalias <name>        remove alias");
        let _ = writeln!(out, "    which <cmd>           locate command in PATH");
        let _ = writeln!(out, "    type <cmd>            describe command type");
        let _ = writeln!(out, "    echo <text>           print text ($VAR, $?, $$ expansion)");
        let _ = writeln!(out, "");
        let _ = writeln!(out, "  Session:");
        let _ = writeln!(out, "    history [N]           show command history");
        let _ = writeln!(out, "    jobs                  list background jobs");
        let _ = writeln!(out, "    clear                 clear the screen");
        let _ = writeln!(out, "    exit [code]           end session");
        let _ = writeln!(out, "");
        let _ = writeln!(out, "  Operators:");
        let _ = writeln!(out, "    cmd1 ; cmd2           sequential execution");
        let _ = writeln!(out, "    cmd1 && cmd2          run cmd2 only if cmd1 succeeds (exit code 0)");
        let _ = writeln!(out, "    cmd1 || cmd2          run cmd2 only if cmd1 fails (exit code != 0)");
        let _ = writeln!(out, "    cmd1 | cmd2           pipe stdout to stdin");
        let _ = writeln!(out, "    cmd > file            redirect stdout (overwrite)");
        let _ = writeln!(out, "    cmd >> file           redirect stdout (append)");
        let _ = writeln!(out, "    cmd < file            redirect file to stdin");
        let _ = writeln!(out, "    cmd 2> file           redirect stderr");
        let _ = writeln!(out, "    cmd &                 run in background");
        (false, 0)
    } else if cmd == "exit" {
        let code = args.first().and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
        (true, code)
    } else if cmd == "ls" {
        let show_hidden = args.iter().any(|a| a == "-a" || a == "-la" || a == "-al");
        let long_format = args.iter().any(|a| a == "-l" || a == "-la" || a == "-al");
        let target = args.iter().find(|a| !a.starts_with('-')).map(|s| s.as_str()).unwrap_or(".");

        if !is_path_safe(target) {
            let _ = writeln!(err_out, "ls: {}: Permission denied (sandboxed)", target);
            return (false, 1);
        }

        match std::fs::read_dir(target) {
            Ok(entries) => {
                let mut items = Vec::new();
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !show_hidden && name.starts_with('.') {
                        continue;
                    }
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    let display_name = if is_dir { format!("{}/", name) } else { name };
                    items.push((display_name, is_dir, size));
                }
                items.sort_by(|a, b| a.0.cmp(&b.0));

                if long_format {
                    for (name, is_dir, size) in items {
                        let kind = if is_dir { "d" } else { "-" };
                        let _ = writeln!(out, "{}rw-r--r-- {:>8} {}", kind, size, name);
                    }
                } else if !items.is_empty() {
                    let names: Vec<String> = items.into_iter().map(|(n, _, _)| n).collect();
                    let _ = writeln!(out, "{}", names.join("  "));
                }
                (false, 0)
            }
            Err(e) => {
                let _ = writeln!(err_out, "ls: {}: {}", target, e);
                (false, 1)
            }
        }
    } else if cmd == "mkdir" {
        if args.is_empty() {
            let _ = writeln!(err_out, "mkdir: missing operand");
            (false, 1)
        } else {
            let make_parents = args.first().map(|s| s == "-p").unwrap_or(false);
            let paths = if make_parents { &args[1..] } else { &args[..] };
            let mut exit_code = 0;
            for path in paths {
                if !is_path_safe(path) {
                    let _ = writeln!(err_out, "mkdir: {}: Permission denied (sandboxed)", path);
                    exit_code = 1;
                    continue;
                }
                if !check_quota_available() {
                    let _ = writeln!(err_out, "mkdir: {}: Disk quota exceeded (50MB limit)", path);
                    exit_code = 1;
                    continue;
                }
                let result = if make_parents {
                    std::fs::create_dir_all(path)
                } else {
                    std::fs::create_dir(path)
                };
                if let Err(e) = result {
                    let _ = writeln!(err_out, "mkdir: {}: {}", path, e);
                    exit_code = 1;
                }
            }
            (false, exit_code)
        }
    } else if cmd == "rmdir" {
        if args.is_empty() {
            let _ = writeln!(err_out, "rmdir: missing operand");
            (false, 1)
        } else {
            let mut exit_code = 0;
            for path in args {
                if !is_path_safe(path) {
                    let _ = writeln!(err_out, "rmdir: {}: Permission denied (sandboxed)", path);
                    exit_code = 1;
                    continue;
                }
                if let Err(e) = std::fs::remove_dir(path) {
                    let _ = writeln!(err_out, "rmdir: {}: {}", path, e);
                    exit_code = 1;
                }
            }
            (false, exit_code)
        }
    } else if cmd == "cat" {
        let file_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        if file_args.is_empty() || file_args == ["-"] {
            // Read from in_stream
            let mut buffer = [0u8; 4096];
            loop {
                match in_stream.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let _ = out.write_all(&buffer[..n]);
                    }
                    Err(_) => break,
                }
            }
            (false, 0)
        } else {
            let mut exit_code = 0;
            for path in file_args {
                if !is_path_safe(path) {
                    let _ = writeln!(err_out, "cat: {}: Permission denied (sandboxed)", path);
                    exit_code = 1;
                    continue;
                }
                match std::fs::read_to_string(path) {
                    Ok(content) => {
                        let _ = write!(out, "{}", content);
                    }
                    Err(e) => {
                        let _ = writeln!(err_out, "cat: {}: {}", path, e);
                        exit_code = 1;
                    }
                }
            }
            (false, exit_code)
        }
    } else if cmd == "touch" {
        if args.is_empty() {
            let _ = writeln!(err_out, "touch: missing operand");
            (false, 1)
        } else {
            let mut exit_code = 0;
            for path in args {
                if !is_path_safe(path) {
                    let _ = writeln!(err_out, "touch: {}: Permission denied (sandboxed)", path);
                    exit_code = 1;
                    continue;
                }
                if !check_quota_available() {
                    let _ = writeln!(err_out, "touch: {}: Disk quota exceeded (50MB limit)", path);
                    exit_code = 1;
                    continue;
                }
                if let Err(e) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                    let _ = writeln!(err_out, "touch: {}: {}", path, e);
                    exit_code = 1;
                }
            }
            (false, exit_code)
        }
    } else if cmd == "rm" {
        if args.is_empty() {
            let _ = writeln!(err_out, "rm: missing operand");
            (false, 1)
        } else {
            let recursive = args.iter().any(|a| a == "-r" || a == "-rf" || a == "-R");
            let force = args.iter().any(|a| a == "-f" || a == "-rf");
            let paths: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
            let mut exit_code = 0;

            for path in paths {
                if !is_path_safe(path) {
                    let _ = writeln!(err_out, "rm: {}: Permission denied (sandboxed)", path);
                    exit_code = 1;
                    continue;
                }
                let target_path = Path::new(path);
                if !target_path.exists() {
                    if !force {
                        let _ = writeln!(err_out, "rm: {}: No such file or directory", path);
                        exit_code = 1;
                    }
                    continue;
                }
                let result = if recursive && target_path.is_dir() {
                    std::fs::remove_dir_all(path)
                } else {
                    std::fs::remove_file(path)
                };
                if let Err(e) = result {
                    if !force {
                        let _ = writeln!(err_out, "rm: {}: {}", path, e);
                        exit_code = 1;
                    }
                }
            }
            (false, exit_code)
        }
    } else if cmd == "cp" {
        let recursive = args.iter().any(|a| a == "-r" || a == "-R");
        let non_flags: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
        if non_flags.len() < 2 {
            let _ = writeln!(err_out, "cp: missing destination file operand");
            (false, 1)
        } else {
            let src = non_flags[0];
            let dst = non_flags[1];
            if !is_path_safe(src) {
                let _ = writeln!(err_out, "cp: {}: Permission denied (sandboxed)", src);
                return (false, 1);
            }
            if !is_path_safe(dst) {
                let _ = writeln!(err_out, "cp: {}: Permission denied (sandboxed)", dst);
                return (false, 1);
            }
            if !check_quota_available() {
                let _ = writeln!(err_out, "cp: {}: Disk quota exceeded (50MB limit)", dst);
                return (false, 1);
            }

            let src_path = Path::new(src);
            if !src_path.exists() {
                let _ = writeln!(err_out, "cp: {}: No such file or directory", src);
                return (false, 1);
            }

            let dst_path = Path::new(dst);
            let final_dst = if dst_path.is_dir() {
                let fname = src_path.file_name().unwrap_or_default();
                dst_path.join(fname)
            } else {
                dst_path.to_path_buf()
            };

            if src_path.is_dir() {
                if !recursive {
                    let _ = writeln!(err_out, "cp: -r not specified; omitting directory '{}'", src);
                    return (false, 1);
                }
                match copy_dir_all(src_path, &final_dst) {
                    Ok(_) => (false, 0),
                    Err(e) => {
                        let _ = writeln!(err_out, "cp: {}: {}", src, e);
                        (false, 1)
                    }
                }
            } else {
                match std::fs::copy(src_path, &final_dst) {
                    Ok(_) => (false, 0),
                    Err(e) => {
                        let _ = writeln!(err_out, "cp: {}: {}", src, e);
                        (false, 1)
                    }
                }
            }
        }
    } else if cmd == "mv" {
        let non_flags: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
        if non_flags.len() < 2 {
            let _ = writeln!(err_out, "mv: missing destination file operand");
            (false, 1)
        } else {
            let src = non_flags[0];
            let dst = non_flags[1];
            if !is_path_safe(src) {
                let _ = writeln!(err_out, "mv: {}: Permission denied (sandboxed)", src);
                return (false, 1);
            }
            if !is_path_safe(dst) {
                let _ = writeln!(err_out, "mv: {}: Permission denied (sandboxed)", dst);
                return (false, 1);
            }

            let src_path = Path::new(src);
            if !src_path.exists() {
                let _ = writeln!(err_out, "mv: {}: No such file or directory", src);
                return (false, 1);
            }

            let dst_path = Path::new(dst);
            let final_dst = if dst_path.is_dir() {
                let fname = src_path.file_name().unwrap_or_default();
                dst_path.join(fname)
            } else {
                dst_path.to_path_buf()
            };

            match std::fs::rename(src_path, &final_dst) {
                Ok(_) => (false, 0),
                Err(_) => {
                    // Fallback to copy and remove across filesystems/devices
                    let copy_res = if src_path.is_dir() {
                        copy_dir_all(src_path, &final_dst)
                    } else {
                        std::fs::copy(src_path, &final_dst).map(|_| ())
                    };
                    match copy_res {
                        Ok(_) => {
                            let _ = if src_path.is_dir() {
                                std::fs::remove_dir_all(src_path)
                            } else {
                                std::fs::remove_file(src_path)
                            };
                            (false, 0)
                        }
                        Err(e) => {
                            let _ = writeln!(err_out, "mv: {}: {}", src, e);
                            (false, 1)
                        }
                    }
                }
            }
        }
    } else if cmd == "head" {
        let mut count: usize = 10;
        let mut file_arg = None;

        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            if arg == "-n" && i + 1 < args.len() {
                if let Ok(c) = args[i + 1].parse::<usize>() {
                    count = c;
                }
                i += 2;
                continue;
            } else if arg.starts_with('-') && arg.len() > 1 && arg[1..].chars().all(|c| c.is_ascii_digit()) {
                if let Ok(c) = arg[1..].parse::<usize>() {
                    count = c;
                }
            } else if !arg.starts_with('-') {
                file_arg = Some(arg.as_str());
            }
            i += 1;
        }

        let lines: Vec<String> = match file_arg {
            Some(path) => {
                if !is_path_safe(path) {
                    let _ = writeln!(err_out, "head: {}: Permission denied (sandboxed)", path);
                    return (false, 1);
                }
                match std::fs::read_to_string(path) {
                    Ok(c) => c.lines().map(|s| s.to_string()).collect(),
                    Err(e) => {
                        let _ = writeln!(err_out, "head: {}: {}", path, e);
                        return (false, 1);
                    }
                }
            }
            None => {
                let mut content = String::new();
                let _ = in_stream.read_to_string(&mut content);
                content.lines().map(|s| s.to_string()).collect()
            }
        };

        for line in lines.iter().take(count) {
            let _ = writeln!(out, "{}", line);
        }
        (false, 0)
    } else if cmd == "tail" {
        let mut count: usize = 10;
        let mut file_arg = None;

        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            if arg == "-n" && i + 1 < args.len() {
                if let Ok(c) = args[i + 1].parse::<usize>() {
                    count = c;
                }
                i += 2;
                continue;
            } else if arg.starts_with('-') && arg.len() > 1 && arg[1..].chars().all(|c| c.is_ascii_digit()) {
                if let Ok(c) = arg[1..].parse::<usize>() {
                    count = c;
                }
            } else if !arg.starts_with('-') {
                file_arg = Some(arg.as_str());
            }
            i += 1;
        }

        let lines: Vec<String> = match file_arg {
            Some(path) => {
                if !is_path_safe(path) {
                    let _ = writeln!(err_out, "tail: {}: Permission denied (sandboxed)", path);
                    return (false, 1);
                }
                match std::fs::read_to_string(path) {
                    Ok(c) => c.lines().map(|s| s.to_string()).collect(),
                    Err(e) => {
                        let _ = writeln!(err_out, "tail: {}: {}", path, e);
                        return (false, 1);
                    }
                }
            }
            None => {
                let mut content = String::new();
                let _ = in_stream.read_to_string(&mut content);
                content.lines().map(|s| s.to_string()).collect()
            }
        };

        let start = lines.len().saturating_sub(count);
        for line in &lines[start..] {
            let _ = writeln!(out, "{}", line);
        }
        (false, 0)
    } else if cmd == "grep" {
        let ignore_case = args.iter().any(|a| a == "-i");
        let invert = args.iter().any(|a| a == "-v");
        let line_numbers = args.iter().any(|a| a == "-n");
        let count_only = args.iter().any(|a| a == "-c");

        let non_flags: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
        if non_flags.is_empty() {
            let _ = writeln!(err_out, "grep: missing pattern");
            return (false, 2);
        }

        let raw_pattern = non_flags[0];
        let pattern = if ignore_case {
            raw_pattern.to_lowercase()
        } else {
            raw_pattern.clone()
        };

        let file_args = &non_flags[1..];
        let mut total_matches = 0;
        let mut had_error = false;

        let check_line = |l: &str| -> bool {
            let matched = if ignore_case {
                l.to_lowercase().contains(&pattern)
            } else {
                l.contains(&pattern)
            };
            if invert { !matched } else { matched }
        };

        if file_args.is_empty() {
            let mut content = String::new();
            let _ = in_stream.read_to_string(&mut content);
            let mut matched_count = 0;
            for (idx, line) in content.lines().enumerate() {
                if check_line(line) {
                    matched_count += 1;
                    total_matches += 1;
                    if !count_only {
                        if line_numbers {
                            let _ = writeln!(out, "{}:{}", idx + 1, line);
                        } else {
                            let _ = writeln!(out, "{}", line);
                        }
                    }
                }
            }
            if count_only {
                let _ = writeln!(out, "{}", matched_count);
            }
        } else {
            let multiple_files = file_args.len() > 1;
            for path in file_args {
                if !is_path_safe(path) {
                    let _ = writeln!(err_out, "grep: {}: Permission denied (sandboxed)", path);
                    had_error = true;
                    continue;
                }
                match std::fs::read_to_string(path) {
                    Ok(content) => {
                        let mut file_matches = 0;
                        for (idx, line) in content.lines().enumerate() {
                            if check_line(line) {
                                file_matches += 1;
                                total_matches += 1;
                                if !count_only {
                                    let prefix = if multiple_files {
                                        format!("{}:", path)
                                    } else {
                                        String::new()
                                    };
                                    if line_numbers {
                                        let _ = writeln!(out, "{}{}:{}", prefix, idx + 1, line);
                                    } else {
                                        let _ = writeln!(out, "{}{}", prefix, line);
                                    }
                                }
                            }
                        }
                        if count_only {
                            if multiple_files {
                                let _ = writeln!(out, "{}:{}", path, file_matches);
                            } else {
                                let _ = writeln!(out, "{}", file_matches);
                            }
                        }
                    }
                    Err(e) => {
                        let _ = writeln!(err_out, "grep: {}: {}", path, e);
                        had_error = true;
                    }
                }
            }
        }

        if had_error {
            (false, 2)
        } else if total_matches > 0 {
            (false, 0)
        } else {
            (false, 1)
        }
    } else if cmd == "wc" {
        let show_lines = args.iter().any(|a| a == "-l");
        let show_words = args.iter().any(|a| a == "-w");
        let show_bytes = args.iter().any(|a| a == "-c");
        let show_all = !show_lines && !show_words && !show_bytes;

        let file_args: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();

        if file_args.is_empty() {
            let mut content = String::new();
            let _ = in_stream.read_to_string(&mut content);
            let lines = content.lines().count();
            let words = content.split_whitespace().count();
            let bytes = content.len();

            if show_all {
                let _ = writeln!(out, "{:>8} {:>8} {:>8}", lines, words, bytes);
            } else {
                let mut parts_out = Vec::new();
                if show_lines { parts_out.push(format!("{:>8}", lines)); }
                if show_words { parts_out.push(format!("{:>8}", words)); }
                if show_bytes { parts_out.push(format!("{:>8}", bytes)); }
                let _ = writeln!(out, "{}", parts_out.join(" "));
            }
            (false, 0)
        } else {
            let mut exit_code = 0;
            let mut tot_lines = 0;
            let mut tot_words = 0;
            let mut tot_bytes = 0;

            for path in &file_args {
                if !is_path_safe(path) {
                    let _ = writeln!(err_out, "wc: {}: Permission denied (sandboxed)", path);
                    exit_code = 1;
                    continue;
                }
                match std::fs::read_to_string(path) {
                    Ok(content) => {
                        let lines = content.lines().count();
                        let words = content.split_whitespace().count();
                        let bytes = content.len();
                        tot_lines += lines;
                        tot_words += words;
                        tot_bytes += bytes;

                        if show_all {
                            let _ = writeln!(out, "{:>8} {:>8} {:>8} {}", lines, words, bytes, path);
                        } else {
                            let mut parts_out = Vec::new();
                            if show_lines { parts_out.push(format!("{:>8}", lines)); }
                            if show_words { parts_out.push(format!("{:>8}", words)); }
                            if show_bytes { parts_out.push(format!("{:>8}", bytes)); }
                            let _ = writeln!(out, "{} {}", parts_out.join(" "), path);
                        }
                    }
                    Err(e) => {
                        let _ = writeln!(err_out, "wc: {}: {}", path, e);
                        exit_code = 1;
                    }
                }
            }

            if file_args.len() > 1 {
                if show_all {
                    let _ = writeln!(out, "{:>8} {:>8} {:>8} total", tot_lines, tot_words, tot_bytes);
                } else {
                    let mut parts_out = Vec::new();
                    if show_lines { parts_out.push(format!("{:>8}", tot_lines)); }
                    if show_words { parts_out.push(format!("{:>8}", tot_words)); }
                    if show_bytes { parts_out.push(format!("{:>8}", tot_bytes)); }
                    let _ = writeln!(out, "{} total", parts_out.join(" "));
                }
            }

            (false, exit_code)
        }
    } else if cmd == "clear" {
        let _ = write!(out, "\x1B[2J\x1B[H");
        (false, 0)
    } else {
        (false, 127)
    }
}

fn split_into_sequences(tokens: &[String]) -> Vec<(String, Vec<String>)> {
    let mut result: Vec<(String, Vec<String>)> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut op = "start".to_string();
    let mut i = 0;

    while i < tokens.len() {
        if tokens[i] == "&&" || tokens[i] == "||" || tokens[i] == ";" {
            result.push((op.clone(), current.clone()));
            op = tokens[i].clone();
            current.clear();
        } else {
            current.push(tokens[i].clone());
        }
        i += 1;
    }
    if !current.is_empty() || !result.is_empty() {
        result.push((op, current));
    }
    result
}

fn main() {
    enable_raw_mode();

    let mut completion_specs: BTreeMap<String, String> = BTreeMap::new();

    let mut jobs: Vec<Job> = Vec::new();
    let mut history: Vec<String> = Vec::new();
    let mut history_appended_index: usize = 0;
    let mut shell_vars: BTreeMap<String, String> = BTreeMap::new();
    let mut exported_vars: BTreeSet<String> = BTreeSet::new();
    let mut aliases: BTreeMap<String, String> = BTreeMap::new();
    let mut last_exit_code: i32 = 0;

    if let Ok(hist_path) = std::env::var("HISTFILE") {
        if let Ok(content) = std::fs::read_to_string(&hist_path) {
            for line in content.lines() {
                if !line.is_empty() {
                    history.push(line.to_string());
                }
            }
        }
    }

    'repl_loop: loop {
        reap_jobs(&mut jobs);

        print!("$ ");
        io::stdout().flush().unwrap();

        let command = read_line_with_completion(&completion_specs, &history);
        let command = command.trim_end_matches(['\n', '\r']);
        if command.is_empty() {
            continue;
        }

        history.push(command.to_string());

        let raw_parts: Vec<String> = parse_command_line(command, &shell_vars, last_exit_code)
            .into_iter()
            .filter(|t| !t.is_empty())
            .collect();

        let sequences = split_into_sequences(&raw_parts);

        for (seq_op, parts) in sequences {
            let should_run = match seq_op.as_str() {
                "&&" => last_exit_code == 0,
                "||" => last_exit_code != 0,
                _ => true,
            };

            if !should_run {
                continue;
            }

            if parts.is_empty() {
                continue;
            }

            #[cfg(unix)]
            if parts.contains(&"|".to_string()) {
                let stages: Vec<Vec<String>> = parts
                    .split(|p| p == "|")
                    .map(|s| s.to_vec())
                    .collect();

                let total_stages = stages.len();
                let mut children = Vec::new();
                let mut prev_reader: Option<std::os::unix::net::UnixStream> = None;

                for (idx, stage) in stages.iter().enumerate() {
                    let is_last = idx == total_stages - 1;

                    let (clean_stage, stdout_redir, stderr_redir, stdin_redir) =
                        parse_redirections(stage);

                    if clean_stage.is_empty() {
                        continue;
                    }

                    let raw_cmd = clean_stage[0].clone();
                    let resolved_cmd = aliases.get(&raw_cmd).cloned().unwrap_or(raw_cmd);
                    let mut resolved_parts = parse_command_line(&resolved_cmd, &shell_vars, last_exit_code);
                    resolved_parts.extend_from_slice(&clean_stage[1..]);

                    if resolved_parts.is_empty() {
                        continue;
                    }

                    let cmd = resolved_parts[0].as_str();
                    let args = &resolved_parts[1..];

                    let (next_reader, next_writer) = if !is_last {
                        let (r, w) = std::os::unix::net::UnixStream::pair().unwrap();
                        (Some(r), Some(w))
                    } else {
                        (None, None)
                    };

                    if is_builtin(cmd) {
                        let mut default_stdin = io::stdin();
                        let mut in_stream: Box<dyn Read> = if let Some(ref path) = stdin_redir {
                            if is_path_safe(path) {
                                if let Ok(f) = File::open(path) {
                                    Box::new(f)
                                } else {
                                    Box::new(io::empty())
                                }
                            } else {
                                eprintln!("{}: {}: Permission denied (sandboxed)", cmd, path);
                                Box::new(io::empty())
                            }
                        } else if let Some(reader) = prev_reader.take() {
                            Box::new(reader)
                        } else {
                            Box::new(&mut default_stdin)
                        };

                        let mut out_stream: Box<dyn Write> = if let Some(ref redir) = stdout_redir {
                            if is_path_safe(&redir.path) {
                                if let Ok(file) = open_file_for_redir(redir) {
                                    Box::new(file)
                                } else {
                                    Box::new(io::stdout())
                                }
                            } else {
                                eprintln!("{}: {}: Permission denied (sandboxed)", cmd, redir.path);
                                Box::new(io::stdout())
                            }
                        } else if let Some(writer) = next_writer {
                            Box::new(writer)
                        } else {
                            Box::new(io::stdout())
                        };

                        let mut err_stream: Box<dyn Write> = if let Some(ref redir) = stderr_redir {
                            if is_path_safe(&redir.path) {
                                if let Ok(file) = open_file_for_redir(redir) {
                                    Box::new(file)
                                } else {
                                    Box::new(io::stderr())
                                }
                            } else {
                                eprintln!("{}: {}: Permission denied (sandboxed)", cmd, redir.path);
                                Box::new(io::stderr())
                            }
                        } else {
                            Box::new(io::stderr())
                        };

                        let (should_exit, exit_code) = execute_builtin(
                            cmd,
                            args,
                            &mut completion_specs,
                            &mut jobs,
                            &mut history,
                            &mut history_appended_index,
                            &mut shell_vars,
                            &mut exported_vars,
                            &mut aliases,
                            in_stream.as_mut(),
                            &mut out_stream,
                            &mut err_stream,
                        );
                        if should_exit {
                            if let Ok(hist_path) = std::env::var("HISTFILE") {
                                save_history_to_file(&hist_path, &history);
                            }
                            std::process::exit(exit_code);
                        }
                        last_exit_code = exit_code;
                    } else if let Some(_path) = get_executable_path(cmd) {
                        let mut child_cmd = Command::new(cmd);
                        child_cmd.args(args);

                        if let Some(reader) = prev_reader.take() {
                            child_cmd.stdin(Stdio::from(OwnedFd::from(reader)));
                        } else if let Some(ref path) = stdin_redir {
                            if is_path_safe(path) {
                                if let Ok(file) = File::open(path) {
                                    child_cmd.stdin(Stdio::from(file));
                                }
                            }
                        } else {
                            child_cmd.stdin(Stdio::inherit());
                        }

                        if let Some(ref redir) = stdout_redir {
                            if is_path_safe(&redir.path) {
                                if let Ok(file) = open_file_for_redir(redir) {
                                    child_cmd.stdout(Stdio::from(OwnedFd::from(file)));
                                }
                            }
                        } else if let Some(writer) = next_writer {
                            child_cmd.stdout(Stdio::from(OwnedFd::from(writer)));
                        }

                        if let Some(ref redir) = stderr_redir {
                            if is_path_safe(&redir.path) {
                                if let Ok(file) = open_file_for_redir(redir) {
                                    child_cmd.stderr(Stdio::from(file));
                                }
                            }
                        }

                        if let Ok(child) = child_cmd.spawn() {
                            children.push(child);
                        }
                        last_exit_code = 0;
                    } else {
                        eprintln!("{}: command not found", cmd);
                        last_exit_code = 127;
                    }

                    prev_reader = next_reader;
                }

                for mut child in children {
                    if let Ok(status) = child.wait() {
                        last_exit_code = status.code().unwrap_or(1);
                    }
                }

                continue;
            }

            let (mut clean_parts, stdout_redir, stderr_redir, stdin_redir) =
                parse_redirections(&parts);

            if clean_parts.is_empty() {
                continue;
            }

            let is_background = clean_parts.last().map(|s| s.as_str()) == Some("&");
            if is_background {
                clean_parts.pop();
            }

            if clean_parts.is_empty() {
                continue;
            }

            let raw_cmd = clean_parts[0].clone();
            let resolved_cmd = aliases.get(&raw_cmd).cloned().unwrap_or(raw_cmd);

            if resolved_cmd != clean_parts[0] {
                let mut new_parts = parse_command_line(&resolved_cmd, &shell_vars, last_exit_code);
                new_parts.extend_from_slice(&clean_parts[1..]);
                clean_parts = new_parts;
            }

            let cmd = clean_parts[0].as_str();
            let args = &clean_parts[1..];

            if is_builtin(cmd) {
                let mut default_stdin = io::stdin();
                let mut in_stream: Box<dyn Read> = if let Some(ref path) = stdin_redir {
                    if is_path_safe(path) {
                        if let Ok(f) = File::open(path) {
                            Box::new(f)
                        } else {
                            eprintln!("{}: {}: No such file or directory", cmd, path);
                            last_exit_code = 1;
                            continue;
                        }
                    } else {
                        eprintln!("{}: {}: Permission denied (sandboxed)", cmd, path);
                        last_exit_code = 1;
                        continue;
                    }
                } else {
                    Box::new(&mut default_stdin)
                };

                let mut out_stream: Box<dyn Write> = if let Some(ref redir) = stdout_redir {
                    if is_path_safe(&redir.path) {
                        if !check_quota_available() {
                            eprintln!("{}: {}: Disk quota exceeded (50MB limit)", cmd, redir.path);
                            last_exit_code = 1;
                            continue;
                        }
                        if let Ok(file) = open_file_for_redir(redir) {
                            Box::new(file)
                        } else {
                            Box::new(io::stdout())
                        }
                    } else {
                        eprintln!("{}: {}: Permission denied (sandboxed)", cmd, redir.path);
                        last_exit_code = 1;
                        continue;
                    }
                } else {
                    Box::new(io::stdout())
                };

                let mut err_stream: Box<dyn Write> = if let Some(ref redir) = stderr_redir {
                    if is_path_safe(&redir.path) {
                        if let Ok(file) = open_file_for_redir(redir) {
                            Box::new(file)
                        } else {
                            Box::new(io::stderr())
                        }
                    } else {
                        eprintln!("{}: {}: Permission denied (sandboxed)", cmd, redir.path);
                        last_exit_code = 1;
                        continue;
                    }
                } else {
                    Box::new(io::stderr())
                };

                let (should_exit, exit_code) = execute_builtin(
                    cmd,
                    args,
                    &mut completion_specs,
                    &mut jobs,
                    &mut history,
                    &mut history_appended_index,
                    &mut shell_vars,
                    &mut exported_vars,
                    &mut aliases,
                    in_stream.as_mut(),
                    &mut out_stream,
                    &mut err_stream,
                );
                if should_exit {
                    break 'repl_loop;
                }
                last_exit_code = exit_code;
            } else if let Some(_path) = get_executable_path(cmd) {
                let mut child_cmd = Command::new(cmd);
                child_cmd.args(args);

                if let Some(ref path) = stdin_redir {
                    if is_path_safe(path) {
                        if let Ok(file) = File::open(path) {
                            child_cmd.stdin(Stdio::from(file));
                        }
                    } else {
                        eprintln!("{}: {}: Permission denied (sandboxed)", cmd, path);
                        last_exit_code = 1;
                        continue;
                    }
                }

                if let Some(ref redir) = stdout_redir {
                    if is_path_safe(&redir.path) {
                        if !check_quota_available() {
                            eprintln!("{}: {}: Disk quota exceeded (50MB limit)", cmd, redir.path);
                            last_exit_code = 1;
                            continue;
                        }
                        if let Ok(file) = open_file_for_redir(redir) {
                            child_cmd.stdout(Stdio::from(file));
                        }
                    } else {
                        eprintln!("{}: {}: Permission denied (sandboxed)", cmd, redir.path);
                        last_exit_code = 1;
                        continue;
                    }
                }

                if let Some(ref redir) = stderr_redir {
                    if is_path_safe(&redir.path) {
                        if let Ok(file) = open_file_for_redir(redir) {
                            child_cmd.stderr(Stdio::from(file));
                        }
                    } else {
                        eprintln!("{}: {}: Permission denied (sandboxed)", cmd, redir.path);
                        last_exit_code = 1;
                        continue;
                    }
                }

                if is_background {
                    if let Ok(child) = child_cmd.spawn() {
                        let pid = child.id();

                        let job_id = if jobs.is_empty() {
                            1
                        } else {
                            jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1
                        };

                        println!("[{}] {}", job_id, pid);

                        let raw_cmd_string = command.trim_end().to_string();
                        jobs.push(Job {
                            id: job_id,
                            pid,
                            command: raw_cmd_string,
                            child,
                        });
                        last_exit_code = 0;
                    }
                } else {
                    match child_cmd.status() {
                        Ok(status) => {
                            last_exit_code = status.code().unwrap_or(1);
                        }
                        Err(_) => { last_exit_code = 1; }
                    }
                }
            } else {
                if let Some(ref redir) = stderr_redir {
                    if is_path_safe(&redir.path) {
                        if let Ok(mut file) = open_file_for_redir(redir) {
                            let _ = writeln!(file, "{}: command not found", cmd);
                        } else {
                            eprintln!("{}: command not found", cmd);
                        }
                    } else {
                        eprintln!("{}: command not found", cmd);
                    }
                } else {
                    eprintln!("{}: command not found", cmd);
                }
                last_exit_code = 127;
            }
        }
    }

    if let Ok(hist_path) = std::env::var("HISTFILE") {
        save_history_to_file(&hist_path, &history);
    }
}

fn get_executable_completions() -> Vec<String> {
    let mut completions = BTreeSet::new();

    completions.insert("echo".to_string());
    completions.insert("exit".to_string());
    completions.insert("complete".to_string());
    completions.insert("jobs".to_string());
    completions.insert("history".to_string());
    completions.insert("declare".to_string());
    completions.insert("ls".to_string());
    completions.insert("cd".to_string());
    completions.insert("mkdir".to_string());
    completions.insert("cat".to_string());
    completions.insert("rm".to_string());
    completions.insert("rmdir".to_string());
    completions.insert("cp".to_string());
    completions.insert("mv".to_string());
    completions.insert("head".to_string());
    completions.insert("tail".to_string());
    completions.insert("grep".to_string());
    completions.insert("wc".to_string());
    completions.insert("touch".to_string());
    completions.insert("clear".to_string());
    completions.insert("export".to_string());
    completions.insert("unset".to_string());
    completions.insert("env".to_string());
    completions.insert("printenv".to_string());
    completions.insert("alias".to_string());
    completions.insert("unalias".to_string());
    completions.insert("which".to_string());
    completions.insert("help".to_string());

    if let Some(path_var) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&path_var) {
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    let full_path = entry.path();
                    if full_path.is_file() {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            if let Ok(metadata) = full_path.metadata() {
                                if metadata.permissions().mode() & 0o111 != 0 {
                                    if let Some(name) = full_path.file_name().and_then(|n| n.to_str()) {
                                        completions.insert(name.to_string());
                                    }
                                }
                            }
                        }
                        #[cfg(not(unix))]
                        {
                            if let Some(name) = full_path.file_name().and_then(|n| n.to_str()) {
                                completions.insert(name.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    completions.into_iter().collect()
}

fn get_path_completions(path_str: &str) -> (String, Vec<String>) {
    let mut completions = BTreeSet::new();

    let (dir_path, prefix) = if let Some(last_slash_idx) = path_str.rfind('/') {
        (&path_str[..=last_slash_idx], &path_str[last_slash_idx + 1..])
    } else {
        ("", path_str)
    };

    let search_dir = if dir_path.is_empty() {
        Path::new(".")
    } else {
        Path::new(dir_path)
    };

    if let Ok(entries) = std::fs::read_dir(search_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with(prefix) {
                    let full_item_path = search_dir.join(name);
                    if full_item_path.is_dir() {
                        completions.insert(format!("{}{}/", dir_path, name));
                    } else {
                        completions.insert(format!("{}{}", dir_path, name));
                    }
                }
            }
        }
    }

    (dir_path.to_string(), completions.into_iter().collect())
}

fn longest_common_prefix(strs: &[&String]) -> String {
    if strs.is_empty() {
        return String::new();
    }
    let first = strs[0].as_str();
    let mut len = first.len();
    for s in strs.iter().skip(1) {
        len = len.min(
            first
                .chars()
                .zip(s.chars())
                .take_while(|(a, b)| a == b)
                .count(),
        );
    }
    first[..len].to_string()
}

fn render_completions(
    buffer: &mut String,
    matches: &[&String],
    replace_len: usize,
    tab_count: &mut usize,
    base_prefix: &str,
    do_common_prefix: bool,
    display_names: Option<&Vec<&str>>,
) {
    *tab_count += 1;
    if matches.is_empty() {
        return;
    }

    if matches.len() == 1 {
        let completion = matches[0];
        let suffix = &completion[replace_len..];
        buffer.push_str(suffix);
        print!("{}", suffix);
        io::stdout().flush().unwrap();
        *tab_count = 0;
        return;
    }

    if do_common_prefix {
        let common = longest_common_prefix(matches);
        if common.len() > replace_len {
            let suffix = &common[replace_len..];
            buffer.push_str(suffix);
            print!("{}", suffix);
            io::stdout().flush().unwrap();
            *tab_count = 0;
            return;
        }
    }

    if *tab_count >= 2 {
        println!();
        if let Some(names) = display_names {
            println!("{}", names.join("  "));
        } else {
            let display: Vec<&str> = matches.iter().map(|s| s.as_str()).collect();
            println!("{}", display.join("  "));
        }
        print!("$ {}{}", base_prefix, buffer);
        io::stdout().flush().unwrap();
        *tab_count = 0;
    }
}

fn run_completer_script(
    script_path: &str,
    command_name: &str,
    word_being_completed: &str,
    previous_word: &str,
    full_line: &str,
) -> Option<Vec<String>> {
    let output = Command::new(script_path)
        .arg(command_name)
        .arg(word_being_completed)
        .arg(previous_word)
        .env("COMP_LINE", full_line)
        .env("COMP_POINT", full_line.len().to_string())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout_str = String::from_utf8(output.stdout).ok()?;
    let mut lines: Vec<String> = stdout_str
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    lines.sort();
    lines.dedup();

    Some(lines)
}

fn read_line_with_completion(
    completion_specs: &BTreeMap<String, String>,
    history: &[String],
) -> String {
    let mut buffer = String::new();
    let mut stdin = io::stdin();
    let mut tab_count = 0;
    let mut history_index = history.len();

    loop {
        let mut byte = [0u8; 1];
        if stdin.read_exact(&mut byte).is_err() {
            break;
        }

        match byte[0] {
            b'\t' => {
                let is_arg_completion = buffer.contains(' ');

                if is_arg_completion {
                    let first_space_idx = buffer.find(' ').unwrap();
                    let command_name = &buffer[..first_space_idx];

                    if let Some(script_path) = completion_specs.get(command_name) {
                        let tokens: Vec<&str> = buffer.split_whitespace().collect();

                        let (word_being_completed, previous_word) = if buffer.ends_with(' ') {
                            ("", tokens.last().copied().unwrap_or(""))
                        } else {
                            let curr = tokens.last().copied().unwrap_or("");
                            let prev = if tokens.len() >= 2 { tokens[tokens.len() - 2] } else { "" };
                            (curr, prev)
                        };

                        let previous_word_str = if previous_word == command_name && tokens.len() == 1 {
                            command_name
                        } else {
                            previous_word
                        };

                        let word_len = word_being_completed.len();
                        if let Some(script_lines) = run_completer_script(
                            script_path,
                            command_name,
                            word_being_completed,
                            previous_word_str,
                            &buffer,
                        ) {
                            let match_refs: Vec<&String> = script_lines.iter().collect();
                            let last_space_idx = buffer.rfind(' ').unwrap();
                            let base_prefix = buffer[..=last_space_idx].to_string();
                            render_completions(
                                &mut buffer,
                                &match_refs,
                                word_len,
                                &mut tab_count,
                                &base_prefix,
                                true,
                                None,
                            );
                            continue;
                        }
                    }

                    let last_space_idx = buffer.rfind(' ').unwrap();
                    let base_prefix = buffer[..=last_space_idx].to_string();
                    let partial_arg = &buffer[last_space_idx + 1..];
                    let partial_arg_len = partial_arg.len();

                    let (_, matches) = get_path_completions(partial_arg);
                    let match_refs: Vec<&String> = matches.iter().collect();
                    let display_names: Vec<&str> = match_refs.iter().map(|s| {
                        let trimmed = s.strip_suffix('/').unwrap_or(s);
                        if let Some(slash_idx) = trimmed.rfind('/') {
                            &s[slash_idx + 1..]
                        } else {
                            s.as_str()
                        }
                    }).collect();

                    render_completions(
                        &mut buffer,
                        &match_refs,
                        partial_arg_len,
                        &mut tab_count,
                        &base_prefix,
                        true,
                        Some(&display_names),
                    );
                } else {
                    let candidates = get_executable_completions();
                    let matches: Vec<&String> = candidates.iter().filter(|c| c.starts_with(&buffer)).collect();
                    let typed_len = buffer.len();

                    render_completions(
                        &mut buffer,
                        &matches,
                        typed_len,
                        &mut tab_count,
                        "",
                        true,
                        None,
                    );
                }
            }
            b'\n' | b'\r' => {
                println!();
                break;
            }
            127 | 8 => {
                tab_count = 0;
                if !buffer.is_empty() {
                    buffer.pop();
                    print!("\r$ {}\x1b[K", buffer);
                    io::stdout().flush().unwrap();
                }
            }
            b'\x1b' => {
                tab_count = 0;
                let mut seq = [0u8; 2];
                if stdin.read_exact(&mut seq).is_ok() {
                    if seq == [b'[', b'A'] {
                        if history_index > 0 {
                            history_index -= 1;
                            buffer = history[history_index].clone();
                            print!("\r$ {}\x1b[K", buffer);
                            io::stdout().flush().unwrap();
                        }
                    } else if seq == [b'[', b'B'] {
                        if history_index + 1 < history.len() {
                            history_index += 1;
                            buffer = history[history_index].clone();
                            print!("\r$ {}\x1b[K", buffer);
                            io::stdout().flush().unwrap();
                        } else if history_index + 1 == history.len() {
                            history_index += 1;
                            buffer.clear();
                            print!("\r$ {}\x1b[K", buffer);
                            io::stdout().flush().unwrap();
                        }
                    }
                }
            }
            b => {
                tab_count = 0;
                buffer.push(b as char);
                print!("{}", b as char);
                io::stdout().flush().unwrap();
            }
        }
    }

    buffer
}

#[cfg(unix)]
fn enable_raw_mode() {
    use std::os::unix::io::AsRawFd;

    #[repr(C)]
    struct Termios {
        c_iflag: u32,
        c_oflag: u32,
        c_cflag: u32,
        c_lflag: u32,
        c_line: u8,
        c_cc: [u8; 32],
        c_ispeed: u32,
        c_ospeed: u32,
    }

    unsafe extern "C" {
        fn tcgetattr(fd: i32, termios_p: *mut Termios) -> i32;
        fn tcsetattr(fd: i32, optional_actions: i32, termios_p: *const Termios) -> i32;
    }

    const ICANON: u32 = 0x00000002;
    const ECHO: u32 = 0x00000008;
    const TCSANOW: i32 = 0;

    unsafe {
        let fd = io::stdin().as_raw_fd();
        let mut termios: Termios = std::mem::zeroed();
        if tcgetattr(fd, &mut termios) == 0 {
            termios.c_lflag &= !(ICANON | ECHO);
            tcsetattr(fd, TCSANOW, &termios);
        }
    }
}

#[cfg(not(unix))]
fn enable_raw_mode() {}

fn open_file_for_redir(redir: &Redirection) -> io::Result<File> {
    match redir.mode {
        RedirectMode::Overwrite => File::create(&redir.path),
        RedirectMode::Append => std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&redir.path),
    }
}

fn parse_command_line(
    input: &str,
    shell_vars: &BTreeMap<String, String>,
    last_exit_code: i32,
) -> Vec<String> {
    let mut args = Vec::new();
    let mut current_arg = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut token_started = false;
    let mut escaped = false;

    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        if escaped {
            current_arg.push(ch);
            token_started = true;
            escaped = false;
            i += 1;
            continue;
        }

        if in_single_quote {
            if ch == '\'' {
                in_single_quote = false;
            } else {
                current_arg.push(ch);
            }
            token_started = true;
            i += 1;
            continue;
        }

        if in_double_quote {
            if ch == '\\' {
                if i + 1 < chars.len() {
                    let next_ch = chars[i + 1];
                    if next_ch == '"' || next_ch == '\\' {
                        current_arg.push(next_ch);
                        token_started = true;
                        i += 2;
                        continue;
                    } else if next_ch == '$' {
                        current_arg.push('$');
                        token_started = true;
                        i += 2;
                        continue;
                    }
                }
                current_arg.push('\\');
                token_started = true;
                i += 1;
                continue;
            } else if ch == '"' {
                in_double_quote = false;
                token_started = true;
                i += 1;
                continue;
            } else if ch == '$' {
                let expanded = expand_var_at(&chars, &mut i, shell_vars, last_exit_code);
                current_arg.push_str(&expanded);
                token_started = true;
                continue;
            } else {
                current_arg.push(ch);
                token_started = true;
                i += 1;
                continue;
            }
        }

        match ch {
            '\\' => {
                escaped = true;
            }
            '\'' => {
                in_single_quote = true;
                token_started = true;
            }
            '"' => {
                in_double_quote = true;
                token_started = true;
            }
            '$' => {
                let expanded = expand_var_at(&chars, &mut i, shell_vars, last_exit_code);
                current_arg.push_str(&expanded);
                token_started = true;
                continue;
            }
            '&' => {
                if token_started {
                    args.push(current_arg.clone());
                    current_arg.clear();
                    token_started = false;
                }
                if i + 1 < chars.len() && chars[i + 1] == '&' {
                    args.push("&&".to_string());
                    i += 2;
                    continue;
                } else {
                    args.push("&".to_string());
                }
            }
            '|' => {
                if token_started {
                    args.push(current_arg.clone());
                    current_arg.clear();
                    token_started = false;
                }
                if i + 1 < chars.len() && chars[i + 1] == '|' {
                    args.push("||".to_string());
                    i += 2;
                    continue;
                } else {
                    args.push("|".to_string());
                }
            }
            ';' => {
                if token_started {
                    args.push(current_arg.clone());
                    current_arg.clear();
                    token_started = false;
                }
                args.push(";".to_string());
            }
            '>' => {
                let is_descriptor_2 = token_started && current_arg == "2";
                let is_descriptor_1 = token_started && current_arg == "1";
                if is_descriptor_2 || is_descriptor_1 {
                    let desc = if is_descriptor_2 { "2" } else { "1" };
                    current_arg.clear();
                    token_started = false;
                    if i + 1 < chars.len() && chars[i + 1] == '>' {
                        args.push(format!("{}>>", desc));
                        i += 2;
                        continue;
                    } else {
                        args.push(format!("{}>", desc));
                    }
                } else {
                    if token_started {
                        args.push(current_arg.clone());
                        current_arg.clear();
                        token_started = false;
                    }
                    if i + 1 < chars.len() && chars[i + 1] == '>' {
                        args.push(">>".to_string());
                        i += 2;
                        continue;
                    } else {
                        args.push(">".to_string());
                    }
                }
            }
            '<' => {
                if token_started {
                    args.push(current_arg.clone());
                    current_arg.clear();
                    token_started = false;
                }
                args.push("<".to_string());
            }
            ' ' | '\t' => {
                if token_started {
                    args.push(current_arg.clone());
                    current_arg.clear();
                    token_started = false;
                }
            }
            _ => {
                current_arg.push(ch);
                token_started = true;
            }
        }

        i += 1;
    }

    if token_started {
        args.push(current_arg);
    }

    args
}

fn get_executable_path(cmd: &str) -> Option<std::path::PathBuf> {
    if let Some(path_var) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&path_var) {
            let full_path = path.join(cmd);
            if full_path.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(metadata) = full_path.metadata() {
                        let mode = metadata.permissions().mode();
                        if mode & 0o111 != 0 {
                            return Some(full_path);
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    return Some(full_path);
                }
            }
        }
    }
    None
}