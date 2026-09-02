//! The state snapshot: one JSON object describing everything the
//! console draws, answered by `/api/state` and by every route that
//! changes something.
//!
//! It is built as a tree of `Serialize` view structs rather than by
//! hand, but the wire format is older than the structs: field order
//! follows declaration order, an optional field is *absent* rather
//! than null unless it was always null (old clients read absence),
//! and floats keep the rendering the hand-written builder gave them —
//! see [`F32`], [`F64`] and [`Fixed`].

use std::collections::BTreeMap;
use std::sync::Mutex;

use aristide_model::units::ratio_to_cents;
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use crate::State;

pub(super) fn state_json(state: &Mutex<State>) -> String {
    state_json_locked(&state.lock().expect("state poisoned"))
}

pub(super) fn state_json_locked(state: &State) -> String {
    serde_json::to_string(&snapshot(state)).expect("the snapshot serializes")
}

// ---- numbers -----------------------------------------------------
//
// The console and the e2e audits read some of these fields as text,
// so how a float renders is part of the wire format, not a detail of
// the serializer.

/// A whole-number check that keeps `i64` in range; every float this
/// module renders is a pitch, a gain or a fraction, far inside it.
fn whole(value: f64) -> Option<i64> {
    (value.is_finite() && value.fract() == 0.0 && value.abs() < 9.0e15).then_some(value as i64)
}

/// An `f64` as `{}` renders it: a whole value without a decimal point
/// (`"hz":415`), anything else at shortest round-trip precision.
#[derive(Clone, Copy)]
pub(super) struct F64(pub f64);

impl Serialize for F64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match whole(self.0) {
            Some(value) => serializer.serialize_i64(value),
            None => serializer.serialize_f64(self.0),
        }
    }
}

/// The same for an `f32` — serialized at `f32` precision, so `0.178`
/// stays `0.178` instead of widening to its `f64` neighbourhood.
#[derive(Clone, Copy)]
pub(super) struct F32(pub f32);

impl Serialize for F32 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match whole(self.0 as f64) {
            Some(value) => serializer.serialize_i64(value),
            None => serializer.serialize_f32(self.0),
        }
    }
}

/// A float the snapshot has always shown at a fixed number of
/// decimals (`{:.2}` for measured pitch, `{:.1}` for a tremulant's
/// rate). The value is rounded to that many places here and then
/// serialized as a float, so the number the console reads is the same
/// one — only a trailing zero is gone (`440.00` renders `440.0`).
#[derive(Clone, Copy)]
pub(super) struct Fixed(f64);

impl Fixed {
    fn new(value: f64, places: i32) -> Self {
        let scale = 10f64.powi(places);
        Fixed((value * scale).round_ties_even() / scale)
    }
}

impl Serialize for Fixed {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.0)
    }
}

// ---- the snapshot ------------------------------------------------

#[derive(Serialize)]
struct Snapshot {
    stops: Vec<StopView>,
    couplers: Vec<CouplerView>,
    manuals: Vec<ManualView>,
    trems: Vec<TremView>,
    /// The single-knob field the console renders today: any engaged.
    tremulant: bool,
    generals: Vec<u8>,
    setter: bool,
    /// The combination action beyond the generals: which divisionals
    /// each manual has stored, where the stepper stands, and where the
    /// crescendo pedal stands with what it is adding. Present whenever
    /// an organ is loaded, so the piston rail never has to guess.
    #[serde(skip_serializing_if = "Option::is_none")]
    combinations: Option<CombinationsView>,
    gain: F32,
    #[serde(skip_serializing_if = "Option::is_none")]
    organ: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    loading: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    load_error: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    load_warnings: Vec<String>,
    library: Vec<LibraryView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tuning: Option<TuningView>,
    /// Present whenever an organ is loaded, `null` when no pipe could
    /// be measured — the one field that is null rather than absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    home: Option<Option<HomeView>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    manual_tuning: Vec<ScopedTuningView>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    source_tuning: Vec<SourceTuningView>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stop_tuning: Vec<StopTuningView>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rank_tuning: Vec<RankTuningView>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    source_home: Vec<SourceHomeView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reverb: Option<F32>,
    midi: MidiView,
    controls: Vec<ControlView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    control_learning: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conflict: Option<ConflictView>,
    actions: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keyboard: Option<KeyboardView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    coupled_keys: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    coupler_repitch: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    noises: Option<NoisesView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enclosures: Option<Vec<EnclosureView>>,
    /// Only panels a player has explicitly placed; anything absent
    /// auto-layouts on the canvas.
    #[serde(skip_serializing_if = "Option::is_none")]
    layout: Option<BTreeMap<String, PanelView>>,
    /// How this instrument was put together: the setup dialog opens on
    /// `implicit` (combined on the CLI, nothing on disk yet), the Organ
    /// preferences edit compasses, and saving writes it all to a file.
    #[serde(skip_serializing_if = "Option::is_none")]
    setup: Option<SetupView>,
}

/// The combination action's live state, for the piston rail.
#[derive(Serialize)]
struct CombinationsView {
    /// Divisionals with something stored: manual index → piston slots.
    divisionals: BTreeMap<usize, Vec<u8>>,
    /// Where the stepper stands, 1-based, and how many frames there
    /// are — `frame` is 0 when the sequence is empty.
    frame: usize,
    frames: usize,
    /// Where the crescendo pedal stands (0 = heel) and how far it goes.
    crescendo: u8,
    crescendo_stages: u8,
    /// Which stages have anything stored — the rail's little ladder.
    crescendo_stored: Vec<u8>,
}

#[derive(Serialize)]
struct StopView {
    id: u32,
    name: String,
    manual: String,
    midx: usize,
    enc: Vec<u32>,
    /// Whether the stop is speaking-capable: hand **or** crescendo.
    on: bool,
    /// Whether the crescendo pedal is holding it. `on && !hand` is a
    /// stop the pedal added — the console lights it without drawing
    /// the knob, so the player can see why it is sounding.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    cres: bool,
    /// Whether the player's own hand has it drawn. Absent means it
    /// equals `on`, which is the case for every stop the crescendo
    /// isn't touching.
    #[serde(skip_serializing_if = "Option::is_none")]
    hand: Option<bool>,
    /// Where the stop came from — what the console's stop editor shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    src: Option<SourceView>,
    pitch: PitchView,
    /// The declared knob engraving, only when one is declared — absent
    /// means "engrave the footage the stop speaks at".
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    /// Present only when the stop speaks pipes of its own — shared
    /// (absent) is the default and the organ norm.
    #[serde(skip_serializing_if = "Option::is_none")]
    own_pipes: Option<bool>,
    tuning: StopScopeView,
    ranks: Vec<RankView>,
}

#[derive(Serialize)]
struct SourceView {
    from: String,
    manual: String,
    stop: String,
}

/// `native` is the footage the samples speak at (null for a mixture),
/// `footage` the override in force (null = native), `cents`/`gain` the
/// stop's own trim rule, `own` whether such a rule exists.
#[derive(Serialize)]
struct PitchView {
    native: Option<F64>,
    footage: Option<F64>,
    cents: F64,
    gain: F64,
    own: bool,
}

/// What the stop's tuning resolves to (`scope`: organ | source |
/// division | stop) and what it follows.
#[derive(Serialize)]
struct StopScopeView {
    scope: &'static str,
    follow: &'static str,
}

#[derive(Serialize)]
struct RankView {
    id: u32,
    name: String,
    own: bool,
}

#[derive(Serialize)]
struct CouplerView {
    idx: usize,
    name: String,
    on: bool,
    routes: Vec<RouteView>,
    /// A jamb seat, linked partners, a coupled-keys override and
    /// `hidden` are all present only when they exist, so the common
    /// snapshot stays small and old clients stay right.
    #[serde(skip_serializing_if = "Option::is_none")]
    midx: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    linked: Option<Vec<usize>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keys: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hidden: Option<bool>,
}

/// Routes for the editor popover: manuals as console indexes,
/// defaults expressed by absence (old clients never read this field).
#[derive(Serialize)]
struct RouteView {
    from: Option<usize>,
    to: Option<usize>,
    shift: i16,
    #[serde(skip_serializing_if = "Option::is_none")]
    low: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    high: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unison_off: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repitch: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    own_pipes: Option<bool>,
}

#[derive(Serialize)]
struct ManualView {
    idx: usize,
    name: String,
    first_key: u8,
    key_count: u16,
    pedal: bool,
    kind: &'static str,
    /// Microtonal manuals carry their effective hex layout — declared
    /// or derived, the console just draws it.
    #[serde(skip_serializing_if = "Option::is_none")]
    hex: Option<HexView>,
    /// When a Lumatone map routes to this manual, the map's key
    /// colours in the same extended-key numbering the notes land in.
    #[serde(skip_serializing_if = "Option::is_none")]
    colors: Option<KeyColors>,
    held: Vec<u16>,
    /// The division's display rank: stop and seated-coupler tokens in
    /// jamb order — what the console deals the drawknobs out from.
    #[serde(skip_serializing_if = "Option::is_none")]
    rank: Option<Vec<String>>,
    /// The keys engaged couplers are pulling down (absent when none),
    /// the mechanical-action view beside `held`.
    #[serde(skip_serializing_if = "Option::is_none")]
    coupled: Option<Vec<u16>>,
}

#[derive(Serialize)]
struct HexView {
    rows: u8,
    cols: u8,
    right: i16,
    upright: i16,
    anchor: u16,
}

/// A hex board's key colours as a JSON object keyed by key number,
/// written in key order rather than sorted as text.
struct KeyColors(Vec<(u16, u32)>);

impl Serialize for KeyColors {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, colour) in &self.0 {
            map.serialize_entry(&key.to_string(), &format!("#{colour:06x}"))?;
        }
        map.end()
    }
}

/// A synth tremulant carries its live shape (depth back in the file's
/// pitch-cents vocabulary); a wave tremulant is recorded in the
/// samples and only says so.
#[derive(Serialize)]
struct TremView {
    idx: usize,
    name: String,
    on: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    wave: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rate: Option<Fixed>,
    #[serde(skip_serializing_if = "Option::is_none")]
    depth: Option<Fixed>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ramp: Option<Fixed>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wobble: Option<i64>,
}

#[derive(Serialize)]
struct LibraryView {
    name: String,
    path: String,
}

/// A tuning as its JSON object; a scale rides along when one stands in
/// for the temperament, named for the popover.
#[derive(Serialize)]
struct TuningView {
    temperament: String,
    edo: u16,
    reference: ReferenceView,
    transpose: i8,
    pipes: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    scale: Option<ScaleView>,
}

#[derive(Serialize)]
struct ReferenceView {
    key: u8,
    hz: F64,
}

#[derive(Serialize)]
struct ScaleView {
    scl: String,
    kbm: Option<String>,
    name: String,
    notes: usize,
}

/// What the samples were recorded in, as measured at load — the truth
/// `original` plays and every target retunes from.
#[derive(Serialize)]
struct HomeView {
    a4_hz: Fixed,
    temperament: Option<String>,
    offsets_cents: Vec<Fixed>,
    spread_cents: Fixed,
    measured: usize,
    pipes: usize,
}

/// Divisions tuned apart from the instrument, by manual index —
/// absent manuals follow the shared tuning.
#[derive(Serialize)]
struct ScopedTuningView {
    idx: usize,
    #[serde(flatten)]
    tuning: TuningView,
}

/// Sets tuned apart, by alias.
#[derive(Serialize)]
struct SourceTuningView {
    source: String,
    #[serde(flatten)]
    tuning: TuningView,
}

/// Stops pinned or tuned apart: a pinned stop carries `follow` only, a
/// tuned one its tuning and `follow: "own"`.
#[derive(Serialize)]
#[serde(untagged)]
enum StopTuningView {
    Own {
        stop: u32,
        follow: &'static str,
        #[serde(flatten)]
        tuning: TuningView,
    },
    Follows {
        stop: u32,
        follow: &'static str,
    },
}

/// Ranks tuned apart within their stops.
#[derive(Serialize)]
struct RankTuningView {
    stop: u32,
    rank: u32,
    #[serde(flatten)]
    tuning: TuningView,
}

/// Each set's own recorded pitch, where the instrument holds more than
/// one: what "as recorded" means at set scope.
#[derive(Serialize)]
struct SourceHomeView {
    source: String,
    a4_hz: Fixed,
}

/// MIDI, as the dialog reads it: the inputs this machine has, and what
/// each of the organ's manuals listens to.
#[derive(Serialize)]
struct MidiView {
    ports: Vec<PortView>,
    manuals: Vec<MidiManualView>,
    /// Which key the dialog is still waiting for.
    #[serde(skip_serializing_if = "Option::is_none")]
    learning: Option<LearningView>,
}

#[derive(Serialize)]
struct PortView {
    id: usize,
    name: String,
    /// The computer keyboard is assignable like any device, though no
    /// operating system will ever list it.
    #[serde(rename = "virtual", skip_serializing_if = "Option::is_none")]
    is_virtual: Option<bool>,
}

#[derive(Serialize)]
struct MidiManualView {
    idx: usize,
    name: String,
    inputs: Vec<InputView>,
    /// What the set itself declares, so the dialog can say how far a
    /// widened keyboard is reaching past it.
    native: Option<(i16, i16)>,
}

/// Bindings name a device even while it is unplugged, so `connected`
/// says whether this one is actually there.
#[derive(Serialize)]
struct InputView {
    slot: usize,
    device: String,
    channel: Option<u8>,
    connected: bool,
    low: Option<u8>,
    high: Option<u8>,
    transpose: i8,
    bend: Option<F32>,
    map: Option<String>,
}

#[derive(Serialize)]
struct LearningView {
    manual: usize,
    slot: usize,
    step: &'static str,
}

#[derive(Serialize)]
struct ControlView {
    slot: usize,
    device: String,
    channel: Option<u8>,
    trigger: String,
    action: String,
    manual: Option<String>,
}

/// A bind parked mid-air: the console draws the keep-both / replace /
/// cancel dialog from this, and answers via /api/conflict.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum ConflictView {
    Input {
        device: String,
        channel: Option<u8>,
        manual: String,
        slot: usize,
        existing: Vec<ExistingInputView>,
    },
    Control {
        device: String,
        channel: Option<u8>,
        trigger: String,
        action: String,
        slot: usize,
        existing: Vec<ExistingControlView>,
    },
}

#[derive(Serialize)]
struct ExistingInputView {
    manual: String,
    slot: usize,
    channel: Option<u8>,
}

#[derive(Serialize)]
struct ExistingControlView {
    slot: usize,
    action: String,
}

/// The legend and the Controls note read one assignment; a keyboard
/// confirmed onto two manuals shows its first here, and the MIDI tab
/// lists every row regardless.
#[derive(Serialize)]
struct KeyboardView {
    manual: usize,
    transpose: i8,
    low: u8,
    high: u8,
}

#[derive(Serialize)]
struct NoisesView {
    on: bool,
    vol: F32,
}

#[derive(Serialize)]
struct EnclosureView {
    idx: usize,
    name: String,
    value: F32,
    displayed: bool,
}

/// Size only when the player set one — absent means the panel hugs its
/// content, and old clients never look.
#[derive(Serialize)]
struct PanelView {
    x: F32,
    y: F32,
    #[serde(skip_serializing_if = "Option::is_none")]
    w: Option<F32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    h: Option<F32>,
}

#[derive(Serialize)]
struct SetupView {
    implicit: bool,
    file: Option<String>,
    adopted: bool,
    sources: Vec<LibraryView>,
    compass: Vec<CompassView>,
}

#[derive(Serialize)]
struct CompassView {
    idx: usize,
    low: u8,
    high: u8,
    native_low: u8,
    native_high: u8,
    declared: bool,
}

// ---- building ----------------------------------------------------

fn snapshot(state: &State) -> Snapshot {
    let console = state.console();
    // Precomputed for the couplers and manuals arrays below: where
    // each coupler is seated (a [console.order] `coupler:` entry puts
    // it in that division's jamb), each division's display rank —
    // stops in dealt order with seated couplers interleaved — and the
    // keys engaged couplers are pulling down, filtered through the
    // organ's coupled-keys default and per-coupler overrides.
    let mut coupler_seats: Vec<Option<usize>> = Vec::new();
    let mut division_ranks: Vec<Vec<String>> = Vec::new();
    let mut coupled_keys: Vec<Vec<u16>> = Vec::new();
    if let Some(console) = console {
        let manuals: Vec<(usize, String)> = console
            .manual_states()
            .into_iter()
            .map(|(idx, name, ..)| (idx, name.to_string()))
            .collect();
        let listed_at = |manual: &str, name: &str| -> Option<usize> {
            state
                .stop_order
                .iter()
                .find(|(listed, _)| listed.eq_ignore_ascii_case(manual))
                .and_then(|(_, order)| {
                    order.iter().position(|entry| entry.eq_ignore_ascii_case(name))
                })
        };
        // (listed position, tiebreak) → token; couplers tiebreak 0 so
        // they sit exactly where listed, stops keep dealt order.
        let mut ranks: Vec<Vec<((usize, usize), String)>> = vec![Vec::new(); manuals.len()];
        for (seq, (id, name, manual, manual_index, _)) in
            console.stop_states().into_iter().enumerate()
        {
            let Some(rank) = ranks.get_mut(manual_index) else { continue };
            let pos = listed_at(manual, name).unwrap_or(usize::MAX);
            rank.push(((pos, seq + 1), format!("s{}", id.0)));
        }
        let couplers = console.coupler_states();
        coupler_seats = vec![None; couplers.len()];
        for (index, name, _, available) in couplers {
            if !available {
                continue;
            }
            let token = format!("coupler:{name}");
            for (midx, manual) in &manuals {
                if let Some(pos) = listed_at(manual, &token) {
                    coupler_seats[index] = Some(*midx);
                    if let Some(rank) = ranks.get_mut(*midx) {
                        rank.push(((pos, 0), format!("c{index}")));
                    }
                    break;
                }
            }
        }
        division_ranks = ranks
            .into_iter()
            .map(|mut rank| {
                rank.sort_by_key(|(key, _)| *key);
                rank.into_iter().map(|(_, token)| token).collect()
            })
            .collect();
        let show: Vec<bool> = console
            .coupler_states()
            .iter()
            .map(|(_, name, _, _)| {
                let mode = state
                    .coupler_key_modes
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(name))
                    .map(|(_, mode)| mode.as_str());
                match mode {
                    Some("never") => false,
                    Some("always") => true,
                    _ => state.coupled_keys,
                }
            })
            .collect();
        coupled_keys =
            console.coupled_display_keys(&|index| show.get(index).copied().unwrap_or(false));
    }

    let stops = console.map(|console| {
        // The player's drawknob order ([console.order]): listed stops
        // first in their listed order, the rest after in assembled
        // order — a stable sort per manual, so a stale name simply
        // has no effect. The console renders the array as dealt.
        let mut states = console.stop_states();
        states.sort_by_key(|(_, name, manual, ..)| {
            state
                .stop_order
                .get(*manual)
                .and_then(|order| {
                    order.iter().position(|listed| listed.eq_ignore_ascii_case(name))
                })
                .unwrap_or(usize::MAX)
        });
        states
            .into_iter()
            .map(|(id, name, manual, manual_index, drawn)| {
                let voicing = state.stop_voicing.get(&id).copied().unwrap_or_default();
                let feet = |feet: Option<f64>| feet.filter(|feet| feet.is_finite()).map(F64);
                let (_, scope) = console.stop_tuning_resolved(id);
                StopView {
                    id: id.0,
                    name: name.to_string(),
                    manual: manual.to_string(),
                    // usize::MAX marks a stop on a manual the set hasn't
                    // got — loaders prevent it, but JSON must stay finite.
                    midx: manual_index.min(u32::MAX as usize),
                    enc: console.stop_enclosures(id),
                    on: drawn,
                    cres: console.crescendo_stops().contains(&id),
                    // Sent only when the two layers disagree: the
                    // console reads an absent `hand` as "same as on",
                    // and an older client that ignores it is right
                    // about every stop the pedal isn't holding.
                    hand: (console.is_hand_drawn(id) != drawn).then_some(!drawn),
                    src: state.provenance.get(&id).map(|prov| SourceView {
                        from: prov.source.clone(),
                        manual: prov.source_manual.clone(),
                        stop: prov.source_stop.clone(),
                    }),
                    pitch: PitchView {
                        native: feet(console.stop_native_footage(id)),
                        footage: feet(voicing.feet),
                        cents: F64(voicing.cents),
                        gain: F64(voicing.gain_db),
                        own: state.stop_voicing.contains_key(&id),
                    },
                    label: state.stop_labels.get(&id).cloned(),
                    own_pipes: console.stop_own_pipes(id).then_some(true),
                    tuning: StopScopeView {
                        scope: scope.name(),
                        follow: if console.stop_own_tuning(id).is_some() {
                            "own"
                        } else {
                            console.stop_follow(id).name()
                        },
                    },
                    ranks: console
                        .stop_ranks(id)
                        .iter()
                        .map(|(rank, name)| RankView {
                            id: rank.0,
                            name: name.to_string(),
                            own: console.rank_tuning(id, *rank).is_some(),
                        })
                        .collect(),
                }
            })
            .collect()
    });

    let couplers = console.map(|console| {
        console
            .coupler_states()
            .into_iter()
            .map(|(index, name, engaged, available)| {
                let linked = console.coupler_linked_with(index);
                CouplerView {
                    idx: index,
                    name: name.to_string(),
                    on: engaged,
                    routes: console
                        .coupler_route_views(index)
                        .iter()
                        .map(|route| RouteView {
                            from: route.from,
                            to: route.to,
                            shift: route.shift,
                            low: route.low,
                            high: route.high,
                            unison_off: route.unison_off.then_some(true),
                            scope: match route.scope {
                                aristide_model::CouplerScope::Bass => Some("bass"),
                                aristide_model::CouplerScope::Melody => Some("melody"),
                                aristide_model::CouplerScope::AllKeys => None,
                            },
                            repitch: route.repitch,
                            own_pipes: route.own_pipes.then_some(true),
                        })
                        .collect(),
                    midx: coupler_seats.get(index).copied().flatten(),
                    linked: (!linked.is_empty()).then_some(linked),
                    keys: state
                        .coupler_key_modes
                        .iter()
                        .find(|(key, _)| key.eq_ignore_ascii_case(name))
                        .map(|(_, mode)| mode.clone()),
                    // Present only when off the console, so the common
                    // snapshot stays small and old clients stay right.
                    hidden: (!available).then_some(true),
                }
            })
            .collect()
    });

    let manuals = console.map(|console| {
        console
            .manual_states()
            .into_iter()
            .map(|(idx, name, first_key, key_count, held)| {
                let hex = console.manual_hex(idx);
                let colors = hex.and_then(|_| {
                    let mut colors: Vec<(u16, u32)> = state
                        .midi_ports
                        .iter()
                        .flat_map(|port| port.map_colors(idx))
                        .collect();
                    colors.sort_unstable();
                    colors.dedup_by_key(|(key, _)| *key);
                    (!colors.is_empty()).then_some(KeyColors(colors))
                });
                ManualView {
                    idx,
                    name: name.to_string(),
                    first_key,
                    key_count,
                    pedal: console.manual_pedal(idx),
                    kind: console.manual_kind(idx).as_str(),
                    hex: hex.map(|hex| HexView {
                        rows: hex.rows,
                        cols: hex.cols,
                        right: hex.right,
                        upright: hex.upright,
                        anchor: hex.anchor,
                    }),
                    colors,
                    held,
                    rank: division_ranks.get(idx).cloned(),
                    coupled: coupled_keys
                        .get(idx)
                        .filter(|keys| !keys.is_empty())
                        .cloned(),
                }
            })
            .collect()
    });

    let trems = state
        .trems
        .iter()
        .enumerate()
        .map(|(index, trem)| {
            let kp = aristide_engine::wind::WindParams::default().pitch_exponent as f64;
            let cents = kp * ratio_to_cents(1.0 + trem.params.depth as f64);
            TremView {
                idx: index,
                name: trem.name.clone(),
                on: trem.engaged,
                wave: trem.wave.then_some(true),
                rate: (!trem.wave).then(|| Fixed::new(trem.params.rate_hz as f64, 1)),
                depth: (!trem.wave).then(|| Fixed::new(cents, 1)),
                ramp: (!trem.wave).then(|| Fixed::new(trem.params.ramp_seconds as f64, 2)),
                wobble: (!trem.wave)
                    .then(|| (trem.params.wobble as f64 * 100.0).round_ties_even() as i64),
            }
        })
        .collect();

    let tuning = console.map(|console| tuning_view(&console.tuning()));
    let home = console.map(|console| {
        console.home().map(|home| HomeView {
            a4_hz: Fixed::new(home.a4_hz, 2),
            temperament: home.temperament.map(|t| t.name().to_string()),
            offsets_cents: home
                .offsets_cents
                .iter()
                .map(|cents| Fixed::new(*cents, 2))
                .collect(),
            spread_cents: Fixed::new(home.spread_cents, 2),
            measured: home.measured,
            pipes: home.pipes,
        })
    });
    let mut source_home: Vec<SourceHomeView> = console
        .map(|console| {
            state
                .setup
                .sources
                .iter()
                .filter_map(|(alias, _)| {
                    let home = console.source_home_of(alias)?;
                    Some(SourceHomeView {
                        source: alias.clone(),
                        a4_hz: Fixed::new(home.a4_hz, 2),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if source_home.len() <= 1 {
        source_home.clear();
    }

    let midi = MidiView {
        ports: state
            .midi_ports
            .iter()
            .enumerate()
            .map(|(id, port)| PortView {
                id,
                name: port.name.clone(),
                is_virtual: None,
            })
            .chain(std::iter::once(PortView {
                id: state.midi_ports.len(),
                name: crate::COMPUTER_KEYBOARD.to_string(),
                is_virtual: Some(true),
            }))
            .collect(),
        manuals: console
            .map(|console| {
                console
                    .manual_states()
                    .iter()
                    .map(|(idx, name, ..)| MidiManualView {
                        idx: *idx,
                        name: name.to_string(),
                        inputs: state
                            .manual_inputs(*idx)
                            .iter()
                            .enumerate()
                            .map(|(slot, input)| InputView {
                                slot,
                                device: input.device.clone(),
                                channel: input.channel,
                                connected: input.device == crate::COMPUTER_KEYBOARD
                                    || state
                                        .midi_ports
                                        .iter()
                                        .any(|p| p.name == input.device),
                                low: input.low,
                                high: input.high,
                                transpose: input.transpose,
                                bend: input.bend.map(F32),
                                map: input.map.clone(),
                            })
                            .collect(),
                        native: console.native_compass(*idx),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        learning: state.learn.as_ref().map(|learn| LearningView {
            manual: learn.manual,
            slot: learn.slot,
            step: if learn.heard.is_some() { "high" } else { "low" },
        }),
    };

    Snapshot {
        stops: stops.unwrap_or_default(),
        couplers: couplers.unwrap_or_default(),
        manuals: manuals.unwrap_or_default(),
        trems,
        tremulant: state.trems.iter().any(|t| t.engaged),
        generals: state
            .midi_config
            .organs
            .get(&state.organ_key)
            .map(|organ| organ.generals.keys().copied().collect())
            .unwrap_or_default(),
        setter: state.setter_armed,
        combinations: console.map(|console| {
            let organ = state.midi_config.organs.get(&state.organ_key);
            let names = console.manual_states();
            CombinationsView {
                divisionals: organ
                    .map(|organ| {
                        organ
                            .divisionals
                            .iter()
                            .filter_map(|(manual, slots)| {
                                let index =
                                    names.iter().position(|(_, name, ..)| *name == manual)?;
                                Some((index, slots.keys().copied().collect()))
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                frame: state.stepper_frame + usize::from(state.stepper_frames() > 0),
                frames: state.stepper_frames(),
                crescendo: state.crescendo_stage,
                crescendo_stages: crate::state::CRESCENDO_STAGES,
                crescendo_stored: organ
                    .map(|organ| {
                        organ
                            .crescendo
                            .iter()
                            .filter(|(_, stops)| !stops.is_empty())
                            .map(|(stage, _)| *stage)
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        }),
        gain: F32(state.master_gain),
        organ: console.map(|console| console.organ_name().to_string()),
        // The picker's world: what could be loaded, what is loading
        // now, and why the last attempt failed.
        loading: state.loading.clone(),
        load_error: state.load_error.clone(),
        // What the last load skipped (dangling refs healed over) — the
        // console shows these, or an organ that heals to emptier than
        // its file intends would look like it simply lost its stops.
        load_warnings: state.load_warnings.clone(),
        // Only organs whose files still exist: a deleted set must not
        // linger in Recent as a row that can only fail to load.
        library: state
            .midi_config
            .present()
            .map(|entry| LibraryView {
                name: entry.name.clone(),
                path: entry.path.display().to_string(),
            })
            .collect(),
        tuning,
        home,
        manual_tuning: console
            .map(|console| {
                (0..console.manual_states().len())
                    .filter_map(|manual| {
                        console.manual_tuning(manual).map(|tuning| ScopedTuningView {
                            idx: manual,
                            tuning: tuning_view(&tuning),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        source_tuning: console
            .map(|console| {
                console
                    .source_tunings()
                    .iter()
                    .map(|(alias, tuning)| SourceTuningView {
                        source: alias.clone(),
                        tuning: tuning_view(tuning),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        stop_tuning: console
            .map(|console| {
                console
                    .stop_tunings()
                    .iter()
                    .map(|(stop, follow, own)| match own {
                        Some(tuning) => StopTuningView::Own {
                            stop: stop.0,
                            follow: "own",
                            tuning: tuning_view(tuning),
                        },
                        None => StopTuningView::Follows {
                            stop: stop.0,
                            follow: follow.name(),
                        },
                    })
                    .collect()
            })
            .unwrap_or_default(),
        rank_tuning: console
            .map(|console| {
                console
                    .rank_tunings()
                    .iter()
                    .map(|(stop, rank, tuning)| RankTuningView {
                        stop: stop.0,
                        rank: rank.0,
                        tuning: tuning_view(tuning),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        source_home,
        reverb: state.reverb_wet.map(F32),
        midi,
        // Bindings, the computer keyboard, and the vocabulary a UI can
        // offer — everything a Controls pane needs to draw itself.
        controls: state
            .controls()
            .iter()
            .enumerate()
            .map(|(slot, control)| ControlView {
                slot,
                device: control.device.clone(),
                channel: control.channel,
                trigger: control.trigger.clone(),
                action: control.action.clone(),
                manual: control.manual.clone(),
            })
            .collect(),
        control_learning: state.control_learn.map(|learn| learn.slot),
        conflict: conflict_view(state),
        actions: crate::control::CATALOGUE.to_vec(),
        keyboard: state.keyboard.first().map(|keyboard| KeyboardView {
            manual: keyboard.manual,
            transpose: keyboard.transpose,
            low: keyboard.compass.0,
            high: keyboard.compass.1,
        }),
        // The organ-wide coupled-keys default, for the console's own
        // organ-scoped settings (per-coupler overrides ride each
        // coupler's `keys` field).
        coupled_keys: console.map(|_| state.coupled_keys),
        coupler_repitch: console.map(|console| console.coupler_repitch()),
        noises: console.map(|console| {
            let (enabled, volume) = console.noises();
            NoisesView {
                on: enabled,
                vol: F32(volume),
            }
        }),
        enclosures: console.map(|console| {
            console
                .enclosure_states()
                .into_iter()
                .map(|(index, name, position, displayed)| EnclosureView {
                    idx: index,
                    name,
                    value: F32(position),
                    displayed,
                })
                .collect()
        }),
        layout: console.map(|_| {
            state
                .layout
                .iter()
                .map(|(panel, pos)| {
                    let (w, h) = match (pos.w, pos.h) {
                        (Some(w), Some(h)) => (Some(F32(w)), Some(F32(h))),
                        _ => (None, None),
                    };
                    (
                        panel.clone(),
                        PanelView {
                            x: F32(pos.x),
                            y: F32(pos.y),
                            w,
                            h,
                        },
                    )
                })
                .collect()
        }),
        setup: (!state.setup.sources.is_empty()).then(|| SetupView {
            implicit: state.setup.implicit,
            file: state
                .composite_path
                .as_ref()
                .map(|path| path.display().to_string()),
            adopted: state.setup.adopted,
            sources: state
                .setup
                .sources
                .iter()
                .map(|(label, path)| LibraryView {
                    name: label.clone(),
                    path: path.display().to_string(),
                })
                .collect(),
            compass: state
                .native_compass()
                .iter()
                .enumerate()
                .map(|(manual, own)| {
                    let declared = state.compass_overrides.get(manual).copied().flatten();
                    let (low, high) = declared.unwrap_or(*own);
                    CompassView {
                        idx: manual,
                        low,
                        high,
                        native_low: own.0,
                        native_high: own.1,
                        declared: declared.is_some(),
                    }
                })
                .collect(),
        }),
    }
}

fn tuning_view(tuning: &crate::tuning::Tuning) -> TuningView {
    TuningView {
        temperament: tuning.temperament.name().to_string(),
        edo: tuning.edo,
        reference: ReferenceView {
            key: tuning.reference.key,
            hz: F64(tuning.reference.hz),
        },
        transpose: tuning.transpose,
        pipes: tuning.pipes.name().to_string(),
        scale: tuning.scale.as_ref().map(|scale| ScaleView {
            scl: scale.scl.clone(),
            kbm: scale.kbm.clone(),
            name: scale.name().to_string(),
            notes: scale.scale.len(),
        }),
    }
}

fn conflict_view(state: &State) -> Option<ConflictView> {
    let pending = state.pending.as_ref()?;
    let names = state.manual_names();
    let name_of =
        |idx: usize| names.get(idx).cloned().unwrap_or_else(|| format!("manual {idx}"));
    Some(match pending {
        crate::Pending::Input {
            manual,
            slot,
            input,
            existing,
        } => ConflictView::Input {
            device: input.device.clone(),
            channel: input.channel,
            manual: name_of(*manual),
            slot: *slot,
            existing: existing
                .iter()
                .map(|(other_manual, other_slot)| ExistingInputView {
                    manual: name_of(*other_manual),
                    slot: *other_slot,
                    channel: state
                        .manual_inputs(*other_manual)
                        .get(*other_slot)
                        .and_then(|row| row.channel),
                })
                .collect(),
        },
        crate::Pending::Control {
            slot,
            control,
            existing,
        } => {
            let controls = state.controls();
            ConflictView::Control {
                device: control.device.clone(),
                channel: control.channel,
                trigger: control.trigger.clone(),
                action: control.action.clone(),
                slot: *slot,
                existing: existing
                    .iter()
                    .map(|other| ExistingControlView {
                        slot: *other,
                        action: controls
                            .get(*other)
                            .map(|c| c.action.clone())
                            .unwrap_or_default(),
                    })
                    .collect(),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire format's numbers, which the console and the e2e
    /// audits read as text: a whole value carries no decimal point, a
    /// fraction keeps its own precision, and a fixed-precision field
    /// rounds to its places but stays a float.
    #[test]
    fn numbers_render_as_the_console_has_always_read_them() {
        assert_eq!(serde_json::to_string(&F64(415.0)).expect("json"), "415");
        assert_eq!(serde_json::to_string(&F64(443.33)).expect("json"), "443.33");
        assert_eq!(serde_json::to_string(&F32(2.0)).expect("json"), "2");
        assert_eq!(serde_json::to_string(&F32(0.178)).expect("json"), "0.178");
        assert_eq!(
            serde_json::to_string(&Fixed::new(443.3312, 2)).expect("json"),
            "443.33"
        );
        assert_eq!(
            serde_json::to_string(&Fixed::new(15.000000123, 1)).expect("json"),
            "15.0"
        );
    }
}
