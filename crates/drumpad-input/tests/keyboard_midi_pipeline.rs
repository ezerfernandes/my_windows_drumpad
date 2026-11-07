//! End-to-end input pipeline: keyboard and MIDI both produce the same
//! `InputEvent` type, unplugging MIDI releases held notes, keyboard fallback
//! needs no restart, and replugging reconnects exactly once.

use std::sync::{Arc, Mutex};

use drumpad_input::keyboard::{KeyboardInput, KeyboardProfile, PhysicalKey};
use drumpad_input::midi::{
    FakeMidiBackend, HotplugEvent, HotplugManager, MidiHandler, MidiOutput, MidiPreferences,
    MidiRouter,
};
use drumpad_input::trigger::{
    EventConsumer, InputEvent, PadId, PadIdExt, TriggerSource, event_queue,
};

fn drain(consumer: &EventConsumer) -> Vec<InputEvent> {
    let mut events = Vec::new();
    while let Some(event) = consumer.pop() {
        events.push(event);
    }
    events
}

struct Harness {
    manager: HotplugManager<FakeMidiBackend>,
    router: Arc<Mutex<MidiRouter>>,
    producer: Arc<Mutex<drumpad_input::trigger::EventProducer>>,
    consumer: EventConsumer,
}

fn harness() -> Harness {
    let (producer, consumer) = event_queue(256);
    let producer = Arc::new(Mutex::new(producer));
    let router = Arc::new(Mutex::new(MidiRouter::nitro_max()));
    let factory = {
        let producer = Arc::clone(&producer);
        let router = Arc::clone(&router);
        move || -> MidiHandler {
            let producer = Arc::clone(&producer);
            let router = Arc::clone(&router);
            let mut scratch = Vec::with_capacity(16);
            Box::new(move |timestamp_us, bytes| {
                scratch.clear();
                router.lock().expect("router lock").process_bytes(
                    bytes,
                    timestamp_us,
                    &mut scratch,
                );
                let producer = producer.lock().expect("producer lock");
                for output in &scratch {
                    if let Some(event) = output.event() {
                        let _ = producer.push(event);
                    }
                }
            })
        }
    };
    let preferences = MidiPreferences {
        device_name: Some("Alesis Nitro Max".to_owned()),
        ..MidiPreferences::default()
    };
    let manager = HotplugManager::new(
        FakeMidiBackend::new(["Alesis Nitro Max"]),
        preferences,
        factory,
    );
    Harness {
        manager,
        router,
        producer,
        consumer,
    }
}

#[test]
fn keyboard_and_midi_share_the_event_type_and_queue() {
    let mut harness = harness();
    let events = harness.manager.poll().unwrap();
    assert!(matches!(
        events.first(),
        Some(HotplugEvent::Connected { .. })
    ));

    // MIDI hit.
    harness
        .manager
        .backend()
        .deliver("Alesis Nitro Max", 10, &[0x99, 36, 100]);
    let midi_events = drain(&harness.consumer);
    assert_eq!(midi_events.len(), 1);
    let InputEvent::Trigger(midi) = midi_events[0] else {
        panic!("expected MIDI trigger, got {midi_events:?}");
    };
    assert_eq!(midi.pad, PadId::from_index(0));
    assert_eq!(midi.source, TriggerSource::Midi);

    // Keyboard hit on the same pad.
    let mut keyboard =
        KeyboardInput::default_with_profile(&KeyboardProfile::default_full_keyboard());
    let action = keyboard.key_down(PhysicalKey::Digit1, 20);
    let keyboard_event = action.event().expect("keyboard trigger");
    harness
        .producer
        .lock()
        .unwrap()
        .push(keyboard_event)
        .unwrap();
    let keyboard_events = drain(&harness.consumer);
    assert_eq!(keyboard_events.len(), 1);
    let InputEvent::Trigger(kb) = keyboard_events[0] else {
        panic!("expected keyboard trigger");
    };
    assert_eq!(kb.pad, midi.pad);
    assert_eq!(kb.source, TriggerSource::Keyboard);
}

#[test]
fn unplug_releases_held_notes_and_keyboard_keeps_working() {
    let mut harness = harness();
    harness.manager.poll().unwrap();

    // Hold a MIDI note and a hi-hat note.
    harness
        .manager
        .backend()
        .deliver("Alesis Nitro Max", 1, &[0x99, 36, 100]);
    harness
        .manager
        .backend()
        .deliver("Alesis Nitro Max", 2, &[0x99, 46, 90]);
    let events = drain(&harness.consumer);
    assert_eq!(events.len(), 2);
    assert_eq!(harness.router.lock().unwrap().held_count(), 2);

    // Unplug: the manager reports the disconnect and the app releases notes.
    harness
        .manager
        .backend_mut()
        .set_ports(Vec::<String>::new());
    let transitions = harness.manager.poll().unwrap();
    assert!(
        transitions
            .iter()
            .any(|event| matches!(event, HotplugEvent::Disconnected { .. }))
    );
    let releases = {
        let mut router = harness.router.lock().unwrap();
        router.release_all(100)
    };
    for release in releases {
        harness.producer.lock().unwrap().push(release).unwrap();
    }
    let released = drain(&harness.consumer);
    assert_eq!(released.len(), 2);
    assert!(released.iter().all(|event| event.is_release()));
    assert!(
        released
            .iter()
            .all(|event| event.source() == TriggerSource::Midi)
    );

    // Keyboard fallback works with no restart and no MIDI connection.
    let mut keyboard =
        KeyboardInput::default_with_profile(&KeyboardProfile::default_full_keyboard());
    let action = keyboard.key_down(PhysicalKey::KeyV, 200);
    harness
        .producer
        .lock()
        .unwrap()
        .push(action.event().unwrap())
        .unwrap();
    let fallback = drain(&harness.consumer);
    assert_eq!(fallback.len(), 1);
    assert_eq!(fallback[0].pad(), PadId::from_index(15));
    assert_eq!(fallback[0].source(), TriggerSource::Keyboard);
}

#[test]
fn replug_reconnects_once_and_routes_again() {
    let mut harness = harness();
    harness.manager.poll().unwrap();
    harness
        .manager
        .backend_mut()
        .set_ports(Vec::<String>::new());
    harness.manager.poll().unwrap();
    assert!(!harness.manager.is_connected());

    harness
        .manager
        .backend_mut()
        .set_ports(["Alesis Nitro Max"]);
    let transitions = harness.manager.poll().unwrap();
    assert!(
        transitions
            .iter()
            .any(|event| matches!(event, HotplugEvent::Connected { .. }))
    );
    harness.manager.poll().unwrap();
    assert_eq!(harness.manager.backend().open_count(), 2);
    assert_eq!(
        harness.manager.backend().open_ports(),
        vec!["Alesis Nitro Max"]
    );

    harness
        .manager
        .backend()
        .deliver("Alesis Nitro Max", 500, &[0x99, 38, 120]);
    let events = drain(&harness.consumer);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].pad(), PadId::from_index(1));
    assert_eq!(events[0].source(), TriggerSource::Midi);
}

#[test]
fn hihat_notes_arrive_as_trigger_events_with_articulation_metadata() {
    let mut harness = harness();
    harness.manager.poll().unwrap();
    {
        let mut router = harness.router.lock().unwrap();
        let mut out = Vec::new();
        router.process_bytes(&[0x99, 44, 100], 1, &mut out);
        assert!(matches!(
            out.as_slice(),
            [MidiOutput::HiHat {
                articulation: drumpad_input::midi::HiHatArticulation::Pedal,
                sound: drumpad_input::midi::HiHatSound::PedalChick,
                ..
            }]
        ));
        let event = out[0].event().unwrap();
        harness.producer.lock().unwrap().push(event).unwrap();
    }
    let events = drain(&harness.consumer);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].pad(), PadId::from_index(9));
    assert_eq!(events[0].source(), TriggerSource::Midi);
}
