// Watcher: turns to face the player and floats gently.
// Runs on the "watcher" object in the plaza, through its Script behavior.

// Once, when the game starts. `this` is this object's own memory.
fn start(object) {
    this.home = object.position;
    this.time = 0.0;
}

// Every frame while the player is near enough for it to be simulated.
fn update(object, dt) {
    this.time += dt;
    let to_player = player().position - object.position;
    object.rotation = vec3(0, atan(-to_player.x, -to_player.z).to_degrees(), 0);
    object.position = this.home + vec3(0, sin(this.time * 2.0) * 0.1, 0);
}
