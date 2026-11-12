# sandbox

A tiny open-source game project that lives inside the engine repo. It is the default project when `--game` is not given, and it is where engine features are tested: grey-box rooms, sample sounds, one of every behavior.

Same layout as any game project:

```
game.ron      game settings: name, start scene
scenes/       RON scenes saved by the editor
prefabs/      prefabs: scenes placed as one object in other scenes
models/       glTF models
audio/        sound clips
textures/     low-res textures
```

Only put content here that is fine to publish (made by us, or CC0 / properly licensed).
