# Roadmap

Work through the milestones in order, one at a time. A milestone is done when its acceptance criteria are met **and the user has confirmed it** (seen, heard or played it). Tick items only then, and don't start the next milestone before that.

At the end of each milestone, tell the user exactly what to run and what to test.

## 0. Repo skeleton
- [x] Cargo workspace (`engine` lib, `app` bin) that builds and runs
- [x] BRIEF.md, CLAUDE.md, ROADMAP.md, DECISIONS.md
- [x] `--game <path>` argument; `sandbox/` as default project; game content lives outside this repo

## 1. Renderer basics
A window that draws simple 3D shapes the way the engine will look.

- [x] winit window + wgpu
- [x] Render at reduced internal resolution (output ÷ integer render-scale factor, default ~540–720 lines), nearest-neighbor upscale to the window; factor changeable live
- [x] Box and other primitive meshes generated in code (enough to build a grey-box level)
- [x] Simple lighting so shapes read: one directional light + ambient, basic shading
- [x] Point-sampled textures; a grid/checker texture for floors and walls (gives a sense of scale and speed)
- [x] Distance fog; tone mapping (AgX or ACES) from the start
- [x] Free-fly debug camera
- [x] egui overlay in the amber terminal style (BRIEF.md → UI style): FPS, internal and output resolution

**Acceptance:** boxes on a grid floor, crisp at any window size, render-scale changes live, 60+ fps.

## 2. First-person controller + test level
A first-person character in a 3D test level, with every movement property tunable live in-game.

- [x] Physics with `rapier3d`; static colliders for the level geometry
- [x] Test level built from boxes (defined in code or a simple RON file; it does not need to be the final scene format). It contains:
    - a large flat floor with the grid texture
    - ramps at 15°, 30°, 45° and 60°
    - stairs with different step heights (10, 20, 30, 40 cm)
    - gap jumps of increasing width and platforms of increasing height, color-coded so distances are recognizable
    - a narrow beam, a low-ceiling tunnel, a tall tower to fall from
    - walls and corners to run into and slide along
- [x] First-person controller (kinematic character controller):
    - walk, sprint, crouch
    - ground acceleration/deceleration, air acceleration and air control
    - jump defined by height and time-to-apex (gravity derived from them), separate fall gravity, max fall speed
    - variable jump height (releasing jump early cuts the jump)
    - coyote time and jump buffering
    - step climbing, max walkable slope, snap to ground
    - mouse look with sensitivity, FOV, optional head bob, landing dip, sprint FOV kick
- [x] All properties in one RON file in the game project: `sandbox/player.ron` (units: meters, seconds, degrees)
- [x] Tuning panel (F1): every property editable live while playing, with Save (writes `player.ron`) and Reset to defaults
- [x] Hot reload: editing `player.ron` by hand applies immediately
- [x] Debug readout: speed, grounded state, velocity, time since grounded
- [x] F2 switches between player and free-fly camera; R respawns at the start

**Acceptance (user plays it):**
- Walking, jumping and landing feel responsive; no snagging on steps or wall edges, no jitter.
- Changing a value in the panel is felt immediately, and saved values load on the next start.
- The user can tune the movement until it feels good.

## 3. Editor loop ← current
- [ ] `--editor` mode with egui, themed per the UI style in BRIEF.md
- [ ] glTF loading
- [ ] Scene format (RON, in the game project's `scenes/`); the test level becomes a scene
- [ ] Click-to-select (raycast), transform gizmos, hierarchy grouped by zone
- [ ] Inspector generated from `bevy_reflect` components
- [ ] Undo/redo via commands
- [ ] Save/load scenes

## 4. Play/Stop in place
- [ ] Play runs the scene inside the editor; Stop restores the edited state
- [ ] PlayerStart object; the player uses the game project's `player.ron`

## 5. First behaviors
- [ ] Floating, Door, Switch, PhysicsBody, SoundSource, Singer, AcousticMaterial
- [ ] Object references in the inspector (switch → door)

## 6. Audio
Steam Audio in the engine, tested by walking through spaces in first person.

- [ ] Add `audionimbus` (with `auto-install`) and an audio output crate (e.g. `cpal`)
- [ ] Test scene with four connected spaces, each with its own acoustic material: a small wooden room (dry), a narrow stone corridor (flutter echoes), a large marble hall (long tail), an open area with no walls
- [ ] Sources: a looping recording at `sandbox/audio/guitar_loop.wav` (fallback: generated tone), a clap on a key at the listener, footsteps depending on the floor material
- [ ] A key moves the looping source into a different space
- [ ] On-screen toggles for A/B comparison: occlusion, reflections/reverb, HRTF, air absorption
- [ ] Simulation on a background thread; master limiter

**Acceptance (user walks through it with headphones):**
- The four spaces clearly sound different, sound is muffled behind walls and opens up through doorways, each toggle makes an audible difference, no clicks or clipping.

## 7. Zones
- [ ] Zone data + connections in scenes
- [ ] Frustum culling, simulation radius
- [ ] Per-zone reverb probes

## 8. Lighting
- [ ] Baked path-traced global illumination into lightmaps per zone ("bake lighting" button)
- [ ] Volumetric fog at reduced resolution: light shafts, lantern halos, per-zone density/color/height; FogVolume behavior
- [ ] Real-time shadows for dynamic lights and moving objects
- [ ] Emissive surfaces lighting the bake and the fog
- [ ] Post-processing: bloom, per-zone color grading (LUT), subtle grain
- [ ] Temporal accumulation for live effects

**Acceptance:** 60+ fps at 4K output with default render scale on an RTX 3060 Ti class GPU; a dark grey-box hall with one window and one lantern looks atmospheric to the user.

## 9. Music system
- [ ] Global key/scale/tempo, motifs quantized to the beat, instrument layers per zone

## 10. Gameplay systems
- [ ] Abilities + gates, save system, map from zone data
