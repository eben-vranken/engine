# CLAUDE.md

Guidance for Claude Code working in this repository.

## What this is

A minimal Rust 3D engine + in-engine editor for quiet, atmospheric, audio-first games: crisp HD, very low-poly worlds, strong lighting and volumetric fog. Signature feature: physically simulated acoustics (Steam Audio via AudioNimbus).

**This repo is the engine only, and it is open source.** Games are separate content-only project folders that live outside this repo; the engine loads one with `--game <path>`.

**Read `BRIEF.md` before doing anything substantial.** It is the source of truth for scope, stack and principles.

## Start of every session

1. Read `docs/ROADMAP.md` and find the current milestone (first unchecked one).
2. Skim `docs/DECISIONS.md` so you don't re-open settled questions.
3. Work only on the current milestone unless the user asks otherwise.

## Repo layout

```
BRIEF.md              what the engine is (source of truth)
CLAUDE.md             this file
docs/ROADMAP.md       milestones with checkboxes + acceptance criteria
docs/DECISIONS.md     decision log (append, never rewrite history)
crates/engine/        core runtime library (render, audio, scene, zones, behaviors)
crates/app/           the single binary: play mode, or editor with --editor
sandbox/              small open-source test project, the default --game
```

New crates are fine when they earn it; keep the workspace small.

## Engine vs. game

- The engine must stay **generic**: nothing specific to any game (names, levels, story, tuned values) goes in engine code. Game-specific choices are data in the game project (`game.ron`, scenes).
- **Never mention a specific game anywhere in this repo**: not in code, comments, docs, commit messages, test names or sample content. Use neutral examples ("a hall", "a door", "a lantern").
- If a game needs something new, add it to the engine as a general feature or behavior, configurable from data.
- Test engine features in `sandbox/`. Only publishable content goes there (our own or CC0 / properly licensed).
- Never copy files from a game project into this repo; game content may be private.

## Commands

```
cargo build
cargo run -p app                                 # play the sandbox
cargo run -p app -- --editor                     # edit the sandbox
cargo run -p app -- --editor --game <path>       # edit another game project
cargo test
cargo clippy --all-targets
cargo fmt
```

Run `cargo fmt`, `cargo clippy --all-targets` and `cargo test` before calling a task done.

## Working rules

- **Minimal first.** Build the smallest thing that satisfies the milestone's acceptance criteria. No speculative abstractions, no features from later milestones.
- **You can't see or hear the result.** Rendering, UI and audio must be checked by the user. After each change that affects them, tell the user exactly what to run and what to look or listen for, then wait for their verdict before building on it.
- **Dependencies:** prefer the stack in `BRIEF.md`. Adding any other crate needs a one-line reason in `docs/DECISIONS.md`. Use `cargo add` so versions are current; don't guess versions from memory.
- **Decisions:** when a real design choice is made (by the user or agreed with them), append it to `docs/DECISIONS.md` with date and reason.
- **Roadmap:** tick a milestone's items in `docs/ROADMAP.md` only after the user has confirmed it works.
- **Ask before** changing anything in `BRIEF.md`, restructuring the workspace, or deleting content in `sandbox/` or any game project.
- **Explain briefly.** The user is a CS student; short explanations of non-obvious engine techniques are welcome, lectures are not.

## Architecture rules (from the brief, non-negotiable without asking)

- One binary. The editor is the game with a UI on top; Play/Stop runs in place.
- Components are plain Rust structs with `bevy_reflect` so the inspector is generated, never hand-written per component.
- Every editor edit goes through an undo-able command.
- Scenes are RON. Models are glTF.
- Every object belongs to a zone; zones and their connections are explicit data.
- Reduced internal resolution: output ÷ integer render-scale factor (default internal height ~540–720 lines), nearest-neighbor upscale. Lighting and fog get the compute this saves.
- Lighting = baked GI + volumetric fog + real-time shadows + post-processing. Live ray-traced effects only where they earn it, at reduced resolution with temporal accumulation.
- Acoustics come from Steam Audio (AudioNimbus). Don't write our own propagation.
- Physics is rapier3d. Gameplay values (movement feel, camera) are data in the game project, never constants in engine code.

## Rust conventions

- Edition 2024, stable toolchain.
- Libraries return `Result` with typed errors (`thiserror` when needed); binaries may use `anyhow`.
- No `unsafe` outside FFI boundaries; none expected since AudioNimbus is a safe wrapper.
- Keep modules small and named after what they do (`audio`, `zones`, `render`), not generic (`utils`, `manager`).

## Gotchas

- AudioNimbus `auto-install` downloads Steam Audio (~180 MB) on the first build; `cargo build` can look stuck while it downloads. It needs Clang 9+ installed.
- The dev profile optimizes dependencies (`opt-level = 3`) so wgpu and audio run at usable speed in debug builds.
