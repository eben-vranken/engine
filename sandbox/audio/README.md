# sandbox audio

Put test recordings here. Only add audio that is fine to publish (own recordings or CC0).

- `guitar_loop.wav`: a looping test source. Mono WAV, 48 kHz, 24-bit, recorded as dry as possible (no reverb or effects), peaks around −6 dB, seamless loop of 10–20 seconds. To use it in `scenes/acoustics.ron`, set the `sound` object's SoundSource clip to `audio/guitar_loop.wav` (an empty clip plays a built-in plucked phrase).
- `clap.wav` (optional): replaces the built-in clap (Q while playing).
- `footsteps/<material>.wav` (optional): replaces the built-in footstep for that floor material, e.g. `footsteps/wood.wav`, `footsteps/marble.wav`, `footsteps/gravel.wav`. Material names are the AcousticMaterial presets in lowercase.

Any sample rate works; stereo files are mixed to mono (every sound is a point in space).