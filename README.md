# engine

A minimal Rust 3D engine and editor for quiet, atmospheric, audio-first games: crisp HD, very low-poly worlds, volumetric light, and physically simulated acoustics.

Working name. See `BRIEF.md` for what this is and `docs/ROADMAP.md` for where it is. Built with Claude Code.

## Run

```
cargo run -p app                                  # play the sandbox
cargo run -p app -- --editor                      # edit the sandbox
cargo run -p app -- --editor --game ../my-game    # edit another game project
```

A game project is a folder of content (settings, scenes, models, audio). It contains no engine code; see `sandbox/` for the layout.

## License

MIT OR Apache-2.0.
