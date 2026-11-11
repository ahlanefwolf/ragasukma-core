<div align="center">

  <img src="icons/128x128@2x.png" alt="Ragasukma Logo" width="120" height="120" />

  # Ragasukma Core
  **The Core Engine for Autonomous File and Project Assistance**

  [![License: MIT](https://img.shields.io/badge/License-MIT-F2B5D4?style=flat-square)](LICENSE)
  [![Donate](https://img.shields.io/badge/Donate-SociaBuzz-FF7043?style=flat-square)](https://sociabuzz.com/ahlanefwolf)

  <p align="center">
    The background engine that navigates your project folders, makes safe file edits,
    verifies work accuracy, and preserves your working history locally.
  </p>

</div>

---

## What is Ragasukma Core?

Ragasukma Core is the background intelligence that executes all assistant tasks. It does not have its own user interface; instead, it acts as the primary engine.

Think of it like a vehicle:
- **Ragasukma Core** is the engine running under the hood.
- Visual elements such as screens, buttons, and controls are provided by interface applications (such as Ragasukma TUI).

---

## Core Capabilities

### Autonomous Task Execution
You give instructions in plain language—such as fixing a bug, adding a new section, or cleaning up existing files. The assistant investigates the problem, executes the work step by step, and ensures the job is fully done.

### Safe File Reading and Editing
The assistant explores your workspace folders, inspects necessary files, searches for exact words or sections, and updates content precisely without touching unrelated parts.

### Self-Verification of Results
Unlike conversational chatbots that only suggest text, this system directly runs checks to verify that every modification works properly and does not break surrounding files.

### Built-in Undo Protection
Every time changes are made, a safety checkpoint is saved. If any outcome does not match what you expected, you can revert all files back to their original state with complete confidence.

### Offline History and Privacy
All conversations, action steps, and task histories are saved directly on your local computer in a single local database file. You can close or power down your machine anytime, then resume exactly where you left off.

### Multi-Account and Multi-Model Support
Connect multiple artificial intelligence services simultaneously, track quota usage across each one, and switch between services seamlessly whenever needed.

### Extensible Skills
Easily plug in additional capabilities—such as web research tools or custom data inspectors—without altering the engine's core operation.

---

## Who is this for?

- **If you just want to use the assistant directly:** You do not need to install or configure this package manually. Simply use **[Ragasukma TUI](../ragasukma-tui)**, which is ready out of the box.
- **If you want to embed this engine into your own custom app:** This core can be integrated directly as the underlying execution driver.

---

## License

Released under the [MIT License](LICENSE) — free to use, modify, and distribute.

Crafted by **Ahlan Saputra** ([@ahlanefwolf](https://github.com/ahlanefwolf)).  
Copyright (c) 2026 Ahlan Saputra.
