// Double jump: one more jump in mid-air, back after landing.
// Turned on by the ABILITY node in the test level's graph (the feather).

// Once, when the ability is turned on. `this` is this script's memory.
fn start() {
    this.used = 0;
}

// Every frame while the ability is on.
fn update(player, dt) {
    if player.grounded {
        this.used = 0;
    }
    // `player.jumped` is the normal jump off the ground: leave that alone.
    if player.pressed("space") && !player.grounded && !player.jumped && this.used < 1 {
        player.jump();
        this.used += 1;
    }
}
