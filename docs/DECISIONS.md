# Decisions

Append new decisions at the bottom. Don't rewrite old entries; supersede them with a new one.

| Date | Decision | Why |
| --- | --- | --- |
| 2026-11-05 | Custom engine on wgpu/winit/egui, not Bevy or Godot | Full control over a small, specialized engine; grow it only as needed |
| 2026-11-05 | Acoustics via Steam Audio through AudioNimbus | Real-time occlusion, reflections, reverb and HRTF with a safe Rust API; pyroomacoustics is offline/Python and only useful as inspiration |
| 2026-11-05 | Own in-engine 3D editor, not Blender-as-editor (Skein) | Wanted a real visual editor; Blender stays for modelling |
| 2026-11-05 | One binary; the editor is the game with a UI on top | No export step, no editor/game mismatch, Play/Stop in place |
| 2026-11-05 | Inspector generated from `bevy_reflect` components | Adding a behavior = writing one struct; keeps the editor maintenance-free |
| 2026-11-05 | Undo/redo via commands from the first editor milestone | Retrofitting undo later means rewriting every tool |
| 2026-11-05 | Scenes in RON, models in glTF | Plain-text, git-diffable scenes; standard model format |
| 2026-11-05 | Crisp HD output with an integer render-scale factor (default internal height ~540–720 lines), nearest-neighbor upscale | Looks HD with a subtle pixel texture; saves 4–9× lighting compute versus native, spent on lighting and fog |
| 2026-11-05 | Lighting = baked path-traced GI + volumetric fog + real-time shadows + post-processing; live ray-traced effects only where they earn it | Atmospheric lighting at 60+ fps; no RTX hardware needed |
| 2026-11-05 | Retro effects (vertex snapping, affine warping, dithering) are optional toggles, off by default | The look comes from low-poly geometry, crisp low-res textures and lighting, not from emulating old hardware |
| 2026-11-05 | Load the whole world at once, but every object belongs to a zone | Low-poly worlds fit in memory; zones enable culling, sim radius, probes, map later |
| 2026-11-05 | Minimal amber terminal-style UI for the editor and default game UI | Desired aesthetic (Blade Runner / Evangelion, but sparser) |
| 2026-11-05 | First milestone is an audio grey-box test | Simulated acoustics are the riskiest and most important feature; prove them before anything else |
| 2026-11-05 | Engine repo is open source and generic; games are separate content-only projects outside this repo, loaded with `--game` | Game content can stay private; keeps game-specific logic out of the engine |
| 2026-11-05 | `sandbox/` test project inside the engine repo, default `--game` | A public place to test engine features |
| 2026-11-05 | No mention of any specific game anywhere in this repo | The engine is a standalone open-source product |
| 2026-11-05 | Supersedes "first milestone is an audio grey-box test": order is renderer → first-person controller + test level → editor → Play/Stop → behaviors → audio | User wants a playable, tunable first-person character in a 3D test level first; audio is tested later inside the 3D engine instead of a throwaway prototype |
| 2026-11-05 | First-person character controller; third-person only if a game needs it | User's choice for the first character |
| 2026-11-05 | Physics with rapier3d | Standard Rust physics crate; provides colliders, a kinematic character controller (steps, slopes, ground snapping) and rigid bodies for PhysicsBody |
| 2026-11-05 | Movement properties in the game project's `player.ron`, tunable live in-game (F1) with save and hot reload | Feel is game-specific data and is tuned by playing, not recompiling |
| 2026-11-05 | Extra crates: `glam` (math), `bytemuck` (GPU buffer casts), `pollster` (block on wgpu's async setup), `log` + `env_logger` (logging), `thiserror` / `anyhow` (errors) | Small, standard companions to wgpu; glam is the common choice around wgpu and is re-exported from `engine` so games share one math type |
| 2026-11-05 | Tone mapping is AgX (polynomial fit), applied in the upscale pass | Handles saturated highlights more gracefully than ACES, suits a moody, low-saturation look |
| 2026-11-05 | Default render-scale factor = ceil(output height / 720) | Matches the brief's table (1080p → 2, 1440p → 2, 4K → 3); manual override 1–8, letterbox for leftover pixels |
| 2026-11-05 | Scene renders to an Rgba16Float target; the window surface is non-sRGB and the upscale shader writes display-encoded values | Keeps HDR until tone mapping; egui also prefers a non-sRGB surface, so both share one target |
| 2026-11-05 | Point-sampled textures still get mipmaps, blended linearly between levels | Crisp texels up close without distant grid lines shimmering |
| 2026-11-05 | Milestone 1's test scene is hard-coded in `crates/app/src/demo.rs` | No scene format until milestone 3; it moves into `sandbox/scenes/` then |
| 2026-11-05 | Engine pins `glam` to the version rapier3d uses (0.33) | rapier 0.36 is glam-based; one `Vec3` type across rendering and physics, no conversions |
| 2026-11-05 | `serde` for (de)serializing RON files | Standard companion to `ron` |
| 2026-11-05 | `player.ron` hot reload polls the file's modified time (every 250 ms) instead of using a file-watcher crate | One file, no new dependency; switch to `notify` when many files need watching |
| 2026-11-05 | Player settings have neutral fallback defaults in engine code; the game's `player.ron` holds the real tuning | A missing or partial file still works and "Reset to defaults" has something to reset to; games never depend on engine constants |
| 2026-11-05 | rapier's `KinematicCharacterController` does collision (slide, autostep, slopes, ground snap); the engine computes the desired motion | Reuses well-tested collide-and-slide; acceleration, jump and camera feel stay ours and data-driven |
| 2026-11-05 | Vertical motion integrates with v·dt + ½·a·dt² per frame, no fixed timestep yet | Exact for constant gravity, so jump height is frame-rate independent (tested at 30–500 fps); add a fixed step when dynamic physics arrives |
| 2026-11-05 | Milestone 2's test level is built in code in `crates/app/src/test_level.rs` | Allowed by the roadmap; moves to a scene file in milestone 3 |
| 2026-11-05 | Supersedes the rapier-collision controller entry: the player is a floating capsule (starts `step_height` above the feet) plus a downward ground probe (small sphere cast, center ray for the walkability normal); rapier's controller is only used to slide the capsule along walls | rapier's autostep failed on capsule-vs-step-edge contacts, and feeding small downward moves into its skin gap sank the player at some frame rates (144 fps); the probe gives smooth stairs and slopes at full speed, tested at 30–2000 fps |
| 2026-11-05 | Horizontal velocity follows the capsule's actual displacement (wall clipping) | Safe now that the floating capsule never touches the ground: any shortfall comes from a wall |
