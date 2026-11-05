//! Single entry point. The editor is the game with a UI on top:
//!
//!   cargo run -p app                                -> play the sandbox
//!   cargo run -p app -- --editor                    -> edit the sandbox
//!   cargo run -p app -- --editor --game ../my-game  -> edit another game project
//!
//! `--game <path>` picks the game project folder (default: `sandbox`).
//! Game projects are content-only folders; they never contain engine code.

mod demo;

use engine::camera::FlyCamera;
use engine::glam::Vec3;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Play,
    Editor,
}

struct Args {
    mode: Mode,
    game: PathBuf,
}

fn parse_args() -> Args {
    let mut mode = Mode::Play;
    let mut game = PathBuf::from("sandbox");
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--editor" => mode = Mode::Editor,
            "--game" => {
                if let Some(path) = args.next() {
                    game = PathBuf::from(path);
                }
            }
            other => eprintln!("ignoring unknown argument: {other}"),
        }
    }

    Args { mode, game }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_hal=error,wgpu_core=warn"),
    )
    .init();
    let args = parse_args();
    log::info!(
        "{} v{} | mode: {:?} | game: {}",
        engine::NAME,
        engine::VERSION,
        args.mode,
        args.game.display()
    );
    if args.mode == Mode::Editor {
        log::warn!("the editor arrives in milestone 3; running play mode");
    }

    engine::run(engine::RunConfig {
        title: format!("{} — {}", engine::NAME, args.game.display()),
        scene: demo::scene(),
        camera: FlyCamera::new(Vec3::new(0.0, 1.7, 6.0), 0.0, -0.08),
    })?;
    Ok(())
}
