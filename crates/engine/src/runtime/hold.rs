//! Carrying things: a `Holdable` the player takes (E) follows the view in
//! front of them until it's put down (the drop key) or swapped for another.
//! While held it collides with nothing; its use key can switch its lights
//! (a flashlight), and scripts on it see `object.held` for anything else.

use super::{CameraMode, Running};
use crate::scene::Transform;
use glam::{EulerRot, Quat, Vec3};

/// The furthest the held object lags behind the view (degrees), so a fast
/// turn never swings it out of sight.
const MAX_LAG: f32 = 30.0;

impl Running {
    /// The object the player holds, if any.
    pub(super) fn held(&self) -> Option<usize> {
        self.scene.objects.iter().position(|o| o.play.held)
    }

    /// The player takes `index`; what they held is put down where it was.
    pub(super) fn take(&mut self, index: usize) {
        if let Some(old) = self.held() {
            let spot = self.scene.objects[index].transform;
            self.put_down(old, spot);
        }
        self.scene.objects[index].play.held = true;
        self.physics.set_hidden(index, true);
        self.hand_turn = None;
        self.place_held(0.0);
    }

    /// Each frame of the game, after the player and scripts moved: the use
    /// and drop keys, then the held object goes where the view is now.
    pub(super) fn update_held(&mut self, dt: f32) {
        let Some(index) = self.held() else {
            return;
        };
        let busy = self.story.as_ref().is_some_and(super::story::Story::busy);
        if self.mode == CameraMode::Player && !busy {
            let keys = &self.settings.keys;
            if self.input.bound_pressed(&keys.drop) {
                self.drop_held(index);
                return;
            }
            let switches = self.scene.objects[index]
                .holdable()
                .is_some_and(|h| h.switches_lights);
            if switches && self.input.bound_pressed(&keys.use_held) {
                let object = &mut self.scene.objects[index];
                object.play.lights_off = !object.play.lights_off;
            }
        }
        self.place_held(dt);
    }

    /// Moves the held object to the hand: `offset` from the eyes, turned
    /// with the view, lagging behind it by its `sway`.
    fn place_held(&mut self, dt: f32) {
        let Some(index) = self.held() else {
            return;
        };
        let Some(holdable) = self.scene.objects[index].holdable().cloned() else {
            return;
        };
        let eye = self.player.camera(&self.settings);
        let view = Quat::from_euler(EulerRot::YXZ, -eye.yaw, eye.pitch, 0.0);
        let turn = match self.hand_turn {
            Some(previous) if holdable.sway > 0.0 && dt > 0.0 => {
                // Catches up in about `sway / 4` seconds.
                let follow = 1.0 - (-dt * 4.0 / holdable.sway).exp();
                let lagging = previous.slerp(view, follow);
                let lag = lagging.angle_between(view);
                let max = MAX_LAG.to_radians();
                if lag > max {
                    view.slerp(lagging, max / lag)
                } else {
                    lagging
                }
            }
            _ => view,
        };
        self.hand_turn = Some(turn);
        let offset = holdable.offset;
        let object = &mut self.scene.objects[index];
        object.transform.position = eye.position + turn * Vec3::new(offset.x, offset.y, -offset.z);
        let hand = Transform {
            rotation: holdable.rotation,
            ..Default::default()
        };
        object.transform.set_quat(turn * hand.quat());
        self.renderer
            .update_object(&self.scene, index, &mut self.assets);
    }

    /// Puts the held object down in front of the player: on what they look
    /// at within reach, else on the floor ahead, standing upright.
    pub(super) fn drop_held(&mut self, index: usize) {
        let eye = self.player.camera(&self.settings);
        let reach = self.settings.body.reach;
        let ahead = eye.forward();
        let spot = match self.physics.surface(eye.position, ahead, reach) {
            Some((point, normal)) => point + normal * 0.05,
            None => eye.position + ahead.with_y(0.0).normalize_or(Vec3::NEG_Z) * reach * 0.5,
        };
        let floor = self
            .physics
            .surface(spot, Vec3::NEG_Y, 10.0)
            .map_or(spot, |(point, _)| point);
        let mut transform = self.scene.objects[index].transform;
        transform.rotation = Vec3::new(0.0, -eye.yaw.to_degrees(), 0.0);
        transform.position = floor;
        transform.position.y += self.height_above_origin(index, &transform);
        self.put_down(index, transform);
    }

    /// The held object goes to `transform`, collides again and stays there,
    /// in the zone it was put down in.
    fn put_down(&mut self, index: usize, transform: Transform) {
        let zone = self.zone_map.zone_at(transform.position);
        let object = &mut self.scene.objects[index];
        object.play.held = false;
        object.transform = transform;
        if let Some(zone) = zone {
            object.zone = self.scene.zones[zone].name.clone();
            self.zone_map.move_object(index, zone);
        }
        if let Some(behaviors) = &mut self.behaviors {
            behaviors.set_rest(index, transform);
        }
        self.hand_turn = None;
        self.physics.place_object(index, &transform);
        self.physics.set_hidden(index, false);
        self.renderer
            .update_object(&self.scene, index, &mut self.assets);
    }

    /// How far the object's origin is above its lowest point, placed with
    /// `transform` (so it can be stood on the floor).
    fn height_above_origin(&mut self, index: usize, transform: &Transform) -> f32 {
        let mut object = self.scene.objects[index].clone();
        object.transform = *transform;
        crate::prefab::world_bounds(&object, &mut self.assets)
            .map_or(0.0, |(min, _)| transform.position.y - min.y)
    }

    /// Leaving the scene: what the player holds stays here, put down where
    /// they stand.
    pub(super) fn leave_held(&mut self) {
        if let Some(index) = self.held() {
            let feet = self.player.feet;
            let mut transform = self.scene.objects[index].transform;
            transform.rotation = Vec3::new(0.0, transform.rotation.y, 0.0);
            transform.position = feet;
            transform.position.y += self.height_above_origin(index, &transform);
            self.put_down(index, transform);
        }
    }
}
