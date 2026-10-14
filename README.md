# Rust Online Shell

A full-stack interactive web terminal built entirely in Rust.

- **Shell binary** (`src/`): POSIX-compatible shell engine with builtins, pipes, job control, history, tab completion, and variable expansion.
- **Backend** (`server/`): Axum WebSocket server that pipes browser input/output to the shell process.
- **Frontend** (`frontend/`): Leptos application compiled to WebAssembly, hosting an xterm.js terminal.

## Quick Start (Local)

```bash
# Build everything
bash build.sh

# Run the server
cargo run -p server
```

Open **http://localhost:3000**

## Deploy for Free

```bash
cargo install cargo-shuttle
cargo shuttle login
cargo shuttle deploy --working-directory server
```

See [DEPLOYMENT.md](DEPLOYMENT.md) for full instructions.
