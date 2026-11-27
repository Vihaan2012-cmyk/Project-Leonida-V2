//! Controllers through gilrs, polled on one background thread that also steps the keyboard and mouse pad.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gilrs::ff::{BaseEffect, BaseEffectType, Effect, EffectBuilder, Repeat, Replay, Ticks};
use gilrs::{Axis, Button, EventType, Gilrs, GilrsBuilder, PowerInfo};

use super::abi::{self, BatteryState, RumbleParams, buttons};
use super::hub::{Hub, PadIdentity};
use super::mapping::pad_from_gilrs;
use crate::trace::note;

const POLL: Duration = Duration::from_millis(2);
const BATTERY_EVERY: Duration = Duration::from_secs(5);

static STARTED: AtomicBool = AtomicBool::new(false);

/// Starts the polling thread once. `LEONIDA_GAMEINPUT_THREAD=0` leaves it off, for tests that call
/// [`Hub::tick`] themselves.
pub fn ensure_running(hub: &Arc<Hub>) {
    if std::env::var("LEONIDA_GAMEINPUT_THREAD").is_ok_and(|v| v == "0") {
        return;
    }
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let hub = hub.clone();
    let spawned = std::thread::Builder::new()
        .name("gameinput".into())
        .spawn(move || run(&hub));
    if spawned.is_err() {
        STARTED.store(false, Ordering::Release);
        note("GameInput: the polling thread could not start".to_string());
    }
}

fn run(hub: &Hub) {
    let mut backend = match Backend::new() {
        Ok(b) => Some(b),
        Err(e) => {
            note(format!(
                "GameInput: no controller backend ({e}); keyboard and mouse only"
            ));
            None
        }
    };
    loop {
        if let Some(b) = &mut backend {
            b.poll(hub);
        }
        hub.tick();
        std::thread::sleep(POLL);
    }
}

pub struct Backend {
    gilrs: Gilrs,
    pads: HashMap<usize, u64>,
    effects: HashMap<u64, Effect>,
    last_battery: Instant,
    warned_triggers: bool,
}

impl Backend {
    pub fn new() -> Result<Backend, Box<gilrs::Error>> {
        let gilrs = GilrsBuilder::new()
            .with_default_filters(false)
            .build()
            .map_err(Box::new)?;
        Ok(Backend {
            gilrs,
            pads: HashMap::new(),
            effects: HashMap::new(),
            last_battery: Instant::now() - BATTERY_EVERY,
            warned_triggers: false,
        })
    }

    fn identity(pad: &gilrs::Gamepad<'_>) -> PadIdentity {
        PadIdentity {
            uuid: pad.uuid(),
            name: pad.name().to_string(),
            vendor: pad.vendor_id().unwrap_or(0),
            product: pad.product_id().unwrap_or(0),
        }
    }

    fn connect(&mut self, hub: &Hub, id: gilrs::GamepadId) {
        let source = usize::from(id) as u64;
        self.pads.insert(usize::from(id), source);
        let Some(pad) = self.gilrs.connected_gamepad(id) else {
            return;
        };
        hub.connect_pad(source, &Self::identity(&pad));
        hub.pad_state(source, pad_state(&pad));
    }

    pub fn poll(&mut self, hub: &Hub) {
        if self.pads.is_empty() && self.gilrs.gamepads().next().is_some() {
            let ids: Vec<_> = self.gilrs.gamepads().map(|(id, _)| id).collect();
            for id in ids {
                self.connect(hub, id);
            }
        }
        let mut touched = Vec::new();
        while let Some(event) = self.gilrs.next_event() {
            let key = usize::from(event.id);
            match event.event {
                EventType::Connected => self.connect(hub, event.id),
                EventType::Disconnected => {
                    if let Some(source) = self.pads.get(&key) {
                        hub.disconnect_pad(*source);
                        self.effects.remove(source);
                    }
                }
                EventType::ButtonPressed(Button::Mode, _) => {
                    hub.guide_button(key as u64, true);
                }
                EventType::ButtonReleased(Button::Mode, _) => {
                    hub.guide_button(key as u64, false);
                }
                EventType::Dropped | EventType::ForceFeedbackEffectCompleted => {}
                _ => {
                    if !touched.contains(&event.id) {
                        touched.push(event.id);
                    }
                }
            }
        }
        for id in touched {
            if let Some(pad) = self.gilrs.connected_gamepad(id) {
                hub.pad_state(usize::from(id) as u64, pad_state(&pad));
            }
        }
        for (source, params) in hub.take_rumble() {
            self.rumble(source, params);
        }
        if self.last_battery.elapsed() >= BATTERY_EVERY {
            self.last_battery = Instant::now();
            for (id, pad) in self.gilrs.gamepads() {
                hub.pad_battery(usize::from(id) as u64, battery(pad.power_info()));
            }
        }
    }

    /// gilrs has the two body motors of an Xbox pad and no trigger motors, so the trigger levels are dropped.
    fn rumble(&mut self, source: u64, params: RumbleParams) {
        self.effects.remove(&source);
        if (params.left_trigger > 0.0 || params.right_trigger > 0.0) && !self.warned_triggers {
            self.warned_triggers = true;
            note("GameInput: impulse-trigger rumble has no gilrs motor and is dropped".to_string());
        }
        let strong = (params.low_frequency * 65535.0) as u16;
        let weak = (params.high_frequency * 65535.0) as u16;
        if strong == 0 && weak == 0 {
            return;
        }
        let Some((id, pad)) = self
            .gilrs
            .gamepads()
            .find(|(id, _)| usize::from(*id) as u64 == source)
        else {
            return;
        };
        if !pad.is_ff_supported() {
            return;
        }
        let schedule = Replay {
            after: Ticks::from_ms(0),
            play_for: Ticks::from_ms(1000),
            with_delay: Ticks::from_ms(0),
        };
        let mut builder = EffectBuilder::new();
        for kind in [
            BaseEffectType::Strong { magnitude: strong },
            BaseEffectType::Weak { magnitude: weak },
        ] {
            builder.add_effect(BaseEffect {
                kind,
                scheduling: schedule,
                ..Default::default()
            });
        }
        let built = builder
            .repeat(Repeat::Infinitely)
            .gamepads(&[id])
            .finish(&mut self.gilrs);
        match built {
            Ok(effect) => {
                if effect.play().is_ok() {
                    self.effects.insert(source, effect);
                }
            }
            Err(e) => note(format!("GameInput: rumble effect failed ({e})")),
        }
    }
}

fn pad_state(pad: &gilrs::Gamepad<'_>) -> abi::GamepadState {
    let mut state = pad_from_gilrs(pad);
    let hat_x = pad.value(Axis::DPadX);
    let hat_y = pad.value(Axis::DPadY);
    if hat_x < -0.5 {
        state.buttons |= buttons::DPAD_LEFT;
    }
    if hat_x > 0.5 {
        state.buttons |= buttons::DPAD_RIGHT;
    }
    if hat_y > 0.5 {
        state.buttons |= buttons::DPAD_UP;
    }
    if hat_y < -0.5 {
        state.buttons |= buttons::DPAD_DOWN;
    }
    state
}

fn battery(info: PowerInfo) -> BatteryState {
    let (status, remaining) = match info {
        PowerInfo::Unknown | PowerInfo::Wired => (abi::BATTERY_NOT_PRESENT, 0.0),
        PowerInfo::Discharging(p) => (abi::BATTERY_DISCHARGING, f32::from(p) / 100.0),
        PowerInfo::Charging(p) => (abi::BATTERY_CHARGING, f32::from(p) / 100.0),
        PowerInfo::Charged => (abi::BATTERY_IDLE, 1.0),
    };
    BatteryState {
        remaining_capacity: remaining,
        full_charge_capacity: if status == abi::BATTERY_NOT_PRESENT {
            0.0
        } else {
            1.0
        },
        status,
        ..Default::default()
    }
}
