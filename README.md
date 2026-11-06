<div align="center">

  <img src="icons/128x128@2x.png" alt="Ragasukma Logo" width="120" height="120" />

  # Ragasukma Core
  **The Engine Behind Your AI Coding Assistant**

  [![License: MIT](https://img.shields.io/badge/License-MIT-F2B5D4?style=flat-square)](LICENSE)
  [![Donate](https://img.shields.io/badge/Donate-SociaBuzz-FF7043?style=flat-square&logo=coffeescript&logoColor=white)](https://sociabuzz.com/ahlanefwolf)

  <p align="center">
    The brain of Ragasukma. Everything the AI needs to read your code, change it,
    and prove the result actually works. Powers <a href="../ragasukma-tui">Ragasukma TUI</a>,
    and can power your own creation too.
  </p>

</div>

---

## 💡 What is Ragasukma Core?

Picture a personal programming assistant that can:

- Read and understand your whole project
- Write and change code when you ask
- Check its own work so nothing breaks
- Undo everything if something goes wrong

**Ragasukma Core is what makes all of that possible.** It's the thinking part — no
screen of its own, like the engine of a car. The steering wheel and dashboard come
from somewhere else (that's [Ragasukma TUI](../ragasukma-tui)).

- **Ragasukma Core** = the engine 🚗
- **Ragasukma TUI** = the dashboard and steering wheel 🕹️

You can put this engine under the hood of anything you build.

---

## ✨ What It Can Do

### 🤖 Gets the Job Done on Its Own
The assistant doesn't just chat — it **works**.
- Tell it what you want in plain words: fix this bug, add that feature, tidy up this code.
- It moves step by step, checking each result before going further.
- If something is unclear, it stops and asks instead of guessing.

### 🛠️ Reads and Edits Your Project
- Looks through your folders and reads what's inside.
- Creates new files and updates existing ones.
- Finds any piece of text across the whole project.
- Runs terminal commands when it genuinely needs to.
- It only ever touches the folders you allow — safe by default.

### ✅ Proves the Work Actually Works
This is what sets Ragasukma apart from a plain chatbot.
- After changing code, it runs your checks by itself.
- If something breaks, it reads the message and tries again.
- It recognises your project type on its own — nothing to configure.

### ↩️ Never Loses Your Code
- Every change is saved like a checkpoint in a game.
- If the assistant takes a wrong turn, rewind to before it started.
- No more "my code is a mess and I can't undo it".

### 🔌 Many AI Brains, Side by Side
- Works with several AI services at once.
- Each one's usage is tracked separately, so costs stay clear.
- Adding another one later is easy.

### 🧩 Grows New Abilities
- Hand the assistant extra powers, like browsing the web or reaching a database.
- Install a new ability and it picks it up straight away.
- There's no limit to what you can plug in next.

### 💾 Remembers Everything
- Conversations, usage, and progress are kept for good.
- Close it today, open it next month — it's all still there.
- Everything lives in one local file that's easy to back up or move.

### 👤 Learns Your Project's Rules
- Each project can carry its own guidebook the assistant reads on its own.
- It picks up your style, your rules, and how your project is organised.
- Handy when you jump between projects that work differently.

---

## 🚀 Getting Started

### If you just want to use the app
You don't need this at all — head straight to **[Ragasukma TUI](../ragasukma-tui)**.
It's the ready-made version. Nothing to set up, no code to write.

### If you want to build your own tool
First, get a working development setup with Rust installed — [here's how](https://rustup.rs).
Then download this project:

```bash
git clone https://github.com/ahlanefwolf/ragasukma-core.git
cd ragasukma-core
cargo build
```

To confirm everything is healthy:

```bash
cargo test
```

You should see every check pass without errors.

### What you'll need

| Requirement | Details |
| :--- | :--- |
| **A Rust setup** | Version 1.85 or newer |
| **Git** | So the assistant can keep track of code changes |
| **Operating system** | Linux, macOS, or Windows |
| **Memory** | Very light — about 100 MB is plenty |

There's no database or server to prepare. All your data goes into a single file that
gets created for you automatically.

---

## 📖 Seeing It In Action

The heart of it is simply this: you hand the assistant a request, and it gets on with
the job until it's finished.

> A single request in, a finished piece of work out.

Everything else — reading files, checking the result, undoing mistakes — happens
automatically behind that one moment.

### Want to dig deeper?

- 📘 **[Code Quality & Testing Guide](docs/code_quality_and_testing_guide.md)** — for anyone looking after this codebase.
- 🖥️ **[Ragasukma TUI](../ragasukma-tui)** — a real, working example built on this engine.

---

## 🤝 Who Is This For?

- **Just want to get coding done?** Use [Ragasukma TUI](../ragasukma-tui) — you'll never need to open this.
- **Building your own assistant?** This engine is ready to drop into your project.
- **Curious how it works?** The code is neatly organised and easy to explore.

---

## 📄 License

Released under the [MIT License](LICENSE) — free to use, modify, and distribute.

Made by **Ahlan Saputra** ([@ahlanefwolf](https://github.com/ahlanefwolf)).
Copyright © 2026 Ahlan Saputra.
