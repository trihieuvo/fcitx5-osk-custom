//! Tracks which kind of input device was used last, by watching evdev devices.
//!
//! Only the time of the last event of each kind is recorded, never the events themselves.
//! It needs read access to `/dev/input/event*` (usually the `input` group).

use std::{
    sync::{
        Once, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use evdev::{AbsoluteAxisCode, Device, EventType, KeyCode, PropType, RelativeAxisCode};

static START: OnceLock<Instant> = OnceLock::new();
static STARTED: Once = Once::new();
/// Milliseconds since `START` plus one of the last touchscreen event, 0 means never.
static LAST_TOUCH: AtomicU64 = AtomicU64::new(0);
/// Same as `LAST_TOUCH`, but for mice, touchpads and keyboards.
static LAST_OTHER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Touch,
    Other,
}

fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

fn classify(device: &Device) -> Option<Kind> {
    // Devices created by ourselves or by input methods must not count as user input.
    let name = device.name().unwrap_or_default();
    if name.contains("fcitx5-osk") || name.contains("Uinput") {
        return None;
    }
    let has_mt = device
        .supported_absolute_axes()
        .is_some_and(|a| a.contains(AbsoluteAxisCode::ABS_MT_POSITION_X));
    if has_mt {
        return Some(if device.properties().contains(PropType::DIRECT) {
            Kind::Touch
        } else {
            // touchpad
            Kind::Other
        });
    }
    if device
        .supported_relative_axes()
        .is_some_and(|r| r.contains(RelativeAxisCode::REL_X))
    {
        return Some(Kind::Other);
    }
    if device
        .supported_keys()
        .is_some_and(|k| k.contains(KeyCode::KEY_A))
    {
        return Some(Kind::Other);
    }
    None
}

async fn watch(device: Device, kind: Kind) {
    let name = device.name().unwrap_or_default().to_string();
    let mut stream = match device.into_event_stream() {
        Ok(stream) => stream,
        Err(e) => {
            tracing::warn!("Unable to watch input device {name}: {e}");
            return;
        }
    };
    let target = match kind {
        Kind::Touch => &LAST_TOUCH,
        Kind::Other => &LAST_OTHER,
    };
    tracing::debug!("Watching input device {name} as {kind:?}");
    loop {
        match stream.next_event().await {
            Ok(event) => {
                if matches!(
                    event.event_type(),
                    EventType::KEY | EventType::RELATIVE | EventType::ABSOLUTE
                ) {
                    target.store(now_ms(), Ordering::Relaxed);
                }
            }
            Err(e) => {
                tracing::debug!("Stop watching input device {name}: {e}");
                return;
            }
        }
    }
}

/// Starts watching input devices. It is safe to call it many times.
pub fn ensure_started() {
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("input-activity".to_string())
            .spawn(|| {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        tracing::warn!("Unable to create a runtime for input activity: {e}");
                        return;
                    }
                };
                rt.block_on(async {
                    let mut count = 0;
                    for (_path, device) in evdev::enumerate() {
                        if let Some(kind) = classify(&device) {
                            tokio::spawn(watch(device, kind));
                            count += 1;
                        }
                    }
                    if count == 0 {
                        tracing::warn!(
                            "No readable input device found, is the user in the `input` group?"
                        );
                    }
                    std::future::pending::<()>().await;
                });
            });
        if let Err(e) = spawned {
            tracing::warn!("Unable to start the input activity thread: {e}");
        }
    });
}

/// Returns true if the latest input event within `window` came from a touchscreen.
pub fn touched_within(window: Duration) -> bool {
    let touch = LAST_TOUCH.load(Ordering::Relaxed);
    if touch == 0 {
        return false;
    }
    let other = LAST_OTHER.load(Ordering::Relaxed);
    now_ms().saturating_sub(touch) <= window.as_millis() as u64 && touch >= other
}
