# Rust Online Shell — Free Deployment Guide

This project is **100% deployment ready**.

---

## 🚀 Recommended: 100% Free Deployment WITHOUT Docker (Render.com)

Render provides a native Rust environment that builds and runs standard Cargo workspaces on Linux x86_64 servers without needing Docker or a credit card.

### Why No Docker is Needed:
- **Frontend (`dist/`)**: The Leptos WASM bundle and `xterm.js` assets are already pre-built in `dist/`. You don't need `trunk` or WASM tooling installed on the cloud server.
- **Backend & Shell**: Render automatically detects Rust (`Cargo.toml`), runs `cargo build --release`, and executes the native Linux binary.
- **WebSockets & HTTPS**: Render provides free automatic SSL certificates (`https://...`) and native WebSocket streaming.

---

### Step 1: Push Project to GitHub

Open PowerShell in your project folder (`rust_shell_full`) and run:

```bash
git init
git add .
git commit -m "Rust online shell deployment ready"
```

Then create a new repository on [GitHub](https://github.com/new) (e.g. `rust-online-shell`) and push your code:

```bash
git remote add origin https://github.com/<YOUR_GITHUB_USERNAME>/<YOUR_REPO_NAME>.git
git branch -M main
git push -u origin main
```

*(Note: `.gitignore` is already set up to exclude `/target` and `/storage`, while keeping the pre-built `dist/` frontend included).*

---

### Step 2: Create a Free Web Service on Render

1. Go to [Render.com](https://render.com/) and sign up or log in (free, no credit card required).
2. Click **New +** in the top dashboard and choose **Web Service**.
3. Select **Build and deploy from a Git repository** and connect your GitHub repository.
4. Fill in the following settings:

| Field | Value |
|---|---|
| **Name** | `rust-online-shell` (or your choice) |
| **Region** | Choose the one closest to you (e.g. Frankfurt, Oregon, Singapore) |
| **Branch** | `main` |
| **Language / Runtime** | `Rust` |
| **Build Command** | `cargo build --release -p rust_shell -p server` |
| **Start Command** | `./target/release/server` |
| **Instance Type** | `Free` (0.1 CPU, 512 MB RAM) |

5. Under **Advanced** -> **Add Environment Variable**, add:

| Key | Value | Notes |
|---|---|---|
| `PORT` | `10000` | Render's default web port |
| `RUST_LOG` | `info` | Server logging |
| `SHELL_BINARY_PATH` | `./target/release/rust_shell` | Points the server directly to the shell executable |

6. Click **Create Web Service**.

---

### Step 3: Access Your Shell

Render will clone your repository, run the build command, and start the server. Within 2–3 minutes, you will see:

```
INFO server: Server running on http://0.0.0.0:10000
```

Your shell is now live at your public URL:
`https://rust-online-shell.onrender.com`

---

## 🐳 Optional: If You Ever Want to Use Docker

You **do not need Docker** for Render. However, if you ever want to deploy to container platforms (like Koyeb or Fly.io), a production-ready `Dockerfile` and `.dockerignore` are already included in the root of this project.

### What is Docker?
Docker packages your code, its compiled binaries, and a lightweight Linux operating system into an isolated "container" image. It guarantees that the software runs identically everywhere.

### How the Included Dockerfile Works:
1. **Build Stage (`rust:1.80-slim`)**: Compiles `rust_shell` and `server` in release mode.
2. **Runtime Stage (`debian:bookworm-slim`)**: Copies only the final compiled binaries (`server` and `rust_shell`) and the `dist/` folder into a tiny ~90MB container image.

If you ever want to test it locally:
```bash
docker build -t rust-online-shell .
docker run -p 3000:3000 rust-online-shell
```
Then visit `http://localhost:3000`.

---

## 📋 Platform & Runtime Behavior

- **Persistent User Sandboxes**: Each browser client generates a unique UUID token stored in `localStorage`. The server isolates each user into `storage/users/<token>/home/`.
- **Sandbox Security**: File manipulation (`touch`, `mkdir`, `rm`, `cp`, `mv`, `cat`) and directory changes (`cd`) cannot escape above the user's home directory.
- **Disk Quota**: Hardened with a 50MB per-user quota to prevent storage exhaustion.
- **Render Free Tier Spin-Down**: On Render's free tier, the instance spins down after 15 minutes of inactivity. When a user opens the page, it spins back up automatically within 30–50 seconds.
