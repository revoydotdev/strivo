//! Every refusal the render contract makes, as a distinct value.
//!
//! Split out of the compiler so this file is the whole of "what can go wrong"
//! and a reader does not have to hold the composition logic in their head to
//! check a variant. The variants are also the compiler's API for policy: each
//! one names the lazy answer it replaces.

use std::fmt;

use crate::time::TimeError;

/// Everything this compiler refuses, as a distinct value.
///
/// Typed because each is a different thing a caller can act on: re-record,
/// re-edit, re-save the sidecar, or file a bug. One opaque "invalid request"
/// would collapse four different bugs into one triage item.
#[derive(Debug, Clone, PartialEq)]
pub enum RenderError {
    /// A revision with no cuts. It renders to nothing, and a plan for it
    /// would satisfy "both callers agree" for free.
    NoCuts,
    /// Todo 22's own refusal, carried verbatim: a non-integral speed output, a
    /// negative coordinate, an inverted or empty interval, an output
    /// discontinuity. **Never re-derived here** — two implementations of "is
    /// this a real media coordinate" is the drift this module exists to end.
    Time { clip: usize, error: TimeError },
    /// A layer or track names a cut index the revision does not carry.
    UnknownCut {
        owner: String,
        cut: usize,
        placed: usize,
    },
    /// Two layers, transitions, tracks or effects share a name.
    DuplicateId { kind: &'static str, id: String },
    /// A cut no track claims. A clip with no bus is a clip that renders into
    /// silence, which is the class of defect this todo exists to close.
    UnroutedClip { cut: usize },
    /// One cut routed to two buses.
    ClipRoutedTwice { cut: usize },
    /// A transition that runs past the end of the output.
    TransitionOutOfRange {
        id: String,
        at: i64,
        end: i64,
        output: i64,
    },
    /// A transition naming a layer that was never declared.
    UnknownLayer { transition: String, layer: String },
    /// A caption cue the composition cannot place: its source coordinate was
    /// cut away, or it sits outside the output entirely.
    CueUnmapped {
        index: usize,
        at: i64,
        error: TimeError,
    },
    /// A burned-in caption with no style document. Burning the default style
    /// would silently substitute a look the user never chose.
    BurnWithoutStyle,
    /// The revision declares no video layer, so there is no picture to render.
    NoVideoLayer,
    /// A sub-mix bus that mixes more signals than the plan has tracks. Emitting
    /// the graph anyway would drop the tracks it cannot reach, which is the
    /// silent-substitution defect this module exists to end.
    BusArity {
        stage: String,
        mixes: usize,
        tracks: usize,
    },
    /// A pre-master tap declared as a stage in the whole-programme chain. That
    /// chain runs on the sum, so the filter would reach every bus; the correct
    /// spelling is the tapped track's own `insert`, which runs before the sum.
    TapInProgrammeChain { id: String, bus: String },
    /// An effect step declared as a loudness stage. Normalisation is
    /// measure-then-apply and lives on `plan.loudness`; as a chain step it would
    /// run a *second*, single-pass normalisation after the measured one.
    LoudnessAsChainStep { id: String },
    /// A burned-in caption with no subtitle document to attach. Distinct from
    /// [`Self::BurnWithoutStyle`]: that one is a missing *style*, this one is a
    /// missing *rendered file*, and the caller fixes them in different places.
    BurnWithoutSubtitle,
    /// A burned-in caption over a video graph that does real work. ffmpeg needs
    /// the graph's output label threaded into the subtitle filter, and the
    /// plan does not own a caller's labels; guessing one would silently drop
    /// either the overlay or the captions. Refused rather than mis-wired.
    BurnOverVideoGraph,
    /// The output path already holds a file, so rendering into it would
    /// destroy an artifact somebody may already have approved. Distinct from
    /// every refusal above: this one is not about the *revision*, it is about
    /// the destination, and the fix is a new artifact rather than a new edit.
    /// A render writes a new path per attempt precisely so this cannot be
    /// reached by accident.
    OutputExists { path: std::path::PathBuf },
    /// The tool reported success and wrote no file at all.
    MissingOutput {
        path: std::path::PathBuf,
        detail: String,
    },
    /// The tool reported success and left a zero-byte file. Not a render: a
    /// container with no stream in it is indistinguishable from a real artifact
    /// by its size alone, so it is refused rather than counted.
    EmptyOutput { path: std::path::PathBuf },
    /// Anything the executor hit: a locator the revision no longer has, a
    /// write that failed, a media tool that exited non-zero. The cause is in
    /// the detail because a caller can only act on the text.
    Execute { detail: String },
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCuts => f.write_str("the revision has no cuts to render"),
            Self::Time { clip, error } => write!(f, "clip {clip}: {error}"),
            Self::UnknownCut { owner, cut, placed } => write!(
                f,
                "{owner} names cut {cut}, which the revision's {placed} placed cuts do not include"
            ),
            Self::DuplicateId { kind, id } => write!(f, "two {kind} are named {id:?}"),
            Self::UnroutedClip { cut } => {
                write!(f, "cut {cut} is on no track, so it would render silent")
            }
            Self::ClipRoutedTwice { cut } => write!(f, "cut {cut} is routed to two buses"),
            Self::TransitionOutOfRange {
                id,
                at,
                end,
                output,
            } => write!(
                f,
                "transition {id:?} runs {at}..{end} us, past the {output} us output"
            ),
            Self::UnknownLayer { transition, layer } => {
                write!(
                    f,
                    "transition {transition:?} names undeclared layer {layer:?}"
                )
            }
            Self::CueUnmapped { index, at, error } => {
                write!(f, "caption cue {index} at {at} us: {error}")
            }
            Self::BurnWithoutStyle => {
                f.write_str("captions are set to burn in but no style document was supplied")
            }
            Self::NoVideoLayer => f.write_str("the revision declares no video layer"),
            Self::BusArity {
                stage,
                mixes,
                tracks,
            } => write!(
                f,
                "{stage} mixes {mixes} signals but the plan routes only {tracks} track(s)"
            ),
            Self::TapInProgrammeChain { id, bus } => write!(
                f,
                "{id:?} declares itself a pre-master tap on bus {bus:?}, but the \
                 programme chain runs on the sum of every bus and cannot reach \
                 one bus alone; put the filter on that track's own insert chain"
            ),
            Self::LoudnessAsChainStep { id } => write!(
                f,
                "{id:?} is a loudness stage in the chain; normalisation is \
                 measure-then-apply and belongs on the plan's loudness target, \
                 where the executor runs it twice"
            ),
            Self::BurnWithoutSubtitle => {
                f.write_str("captions are set to burn in but no subtitle document was supplied")
            }
            Self::BurnOverVideoGraph => f.write_str(
                "captions cannot burn in over a video filter graph yet: the graph's \
                 output label has to be threaded into the subtitle filter",
            ),
            Self::OutputExists { path } => write!(
                f,
                "{} already holds an artifact; render to a new path so the existing \
                 one survives",
                path.display()
            ),
            Self::MissingOutput { path, detail } => write!(
                f,
                "the renderer reported success but wrote nothing to {}: {detail}",
                path.display()
            ),
            Self::EmptyOutput { path } => write!(
                f,
                "{} is zero bytes, which is a container with no stream in it rather \
                 than a render",
                path.display()
            ),
            Self::Execute { detail } => f.write_str(detail),
        }
    }
}

impl std::error::Error for RenderError {}
