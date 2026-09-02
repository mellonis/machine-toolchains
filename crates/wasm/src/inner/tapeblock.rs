//! Tape-block snapshots (`.pmt`/`.tmt`, the MT container) as plain values:
//! decode bytes the CLI wrote, encode bytes the CLI can read. The codec is
//! core's (`docs/formats.md (tape-block snapshot)`); this layer only
//! resolves the per-tape/block alphabet split into the effective glyph
//! table each tape actually uses, and turns the codec's panicking
//! preconditions into errors. The JS-facing contract is `docs/wasm.md
//! (tape blocks)`.

use mtc_core::formats::tapeblock::{TapeBlockFile, TapeSnapshot};

/// One decoded tape: its cells in alphabet indices and the glyph table
/// those indices mean — the tape's own if it carries one, the block's
/// otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tape {
    pub origin: i64,
    pub cells: Vec<u8>,
    pub head: i64,
    pub glyphs: Vec<String>,
}

/// A decoded (or resolved) block: the block-level alphabet and every
/// tape with its effective glyphs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeBlock {
    pub alphabet: Vec<String>,
    pub tapes: Vec<Tape>,
}

/// What `encode` accepts: the decoded shape with the redundancies made
/// optional. A tape without `glyphs` inherits the block alphabet; a block
/// without `alphabet` takes the first tape's `glyphs`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TapeInput {
    pub origin: i64,
    pub cells: Vec<u8>,
    pub head: i64,
    pub glyphs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TapeBlockInput {
    pub alphabet: Option<Vec<String>>,
    pub tapes: Vec<TapeInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TapeBlockError {
    /// The codec refused the bytes (magic, CRC, version, bounds), or the
    /// encoder refused the shape; the message is core's.
    Format(String),
    /// No block alphabet was given and the first tape has no glyphs.
    NoAlphabet,
    NoTapes,
    TooManyTapes(usize),
    /// `tape: None` names the block alphabet.
    EmptyAlphabet {
        tape: Option<usize>,
    },
    AlphabetTooWide {
        tape: Option<usize>,
        symbols: usize,
    },
    GlyphTooLong {
        tape: Option<usize>,
        glyph: String,
    },
    CellOutsideAlphabet {
        tape: usize,
        index: u8,
        width: usize,
    },
}

impl std::fmt::Display for TapeBlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn owner(tape: Option<usize>) -> String {
            match tape {
                None => "the block alphabet".to_string(),
                Some(t) => format!("tape {t}'s glyphs"),
            }
        }
        match self {
            TapeBlockError::Format(m) => write!(f, "{m}"),
            TapeBlockError::NoAlphabet => {
                write!(
                    f,
                    "no alphabet: give the block one, or the first tape `glyphs`"
                )
            }
            TapeBlockError::NoTapes => write!(f, "a tape block needs at least one tape"),
            TapeBlockError::TooManyTapes(n) => write!(f, "{n} tapes; the format holds 255"),
            TapeBlockError::EmptyAlphabet { tape } => write!(f, "{} is empty", owner(*tape)),
            TapeBlockError::AlphabetTooWide { tape, symbols } => {
                write!(
                    f,
                    "{} has {symbols} symbols; the format holds 255",
                    owner(*tape)
                )
            }
            TapeBlockError::GlyphTooLong { tape, glyph } => {
                write!(
                    f,
                    "{}: glyph `{glyph}` is longer than 65535 bytes",
                    owner(*tape)
                )
            }
            TapeBlockError::CellOutsideAlphabet { tape, index, width } => {
                write!(
                    f,
                    "tape {tape}: cell index {index} outside its alphabet of {width}"
                )
            }
        }
    }
}

fn check_glyphs(glyphs: &[String], tape: Option<usize>) -> Result<(), TapeBlockError> {
    if glyphs.is_empty() {
        return Err(TapeBlockError::EmptyAlphabet { tape });
    }
    if glyphs.len() > usize::from(u8::MAX) {
        return Err(TapeBlockError::AlphabetTooWide {
            tape,
            symbols: glyphs.len(),
        });
    }
    if let Some(glyph) = glyphs.iter().find(|g| g.len() > usize::from(u16::MAX)) {
        return Err(TapeBlockError::GlyphTooLong {
            tape,
            glyph: glyph.clone(),
        });
    }
    Ok(())
}

impl TapeBlockInput {
    /// Fills in what was omitted and validates every cell against the
    /// tape's effective glyphs — the block `encode` writes, and the block
    /// `Program::seeds_from_tape_block` maps.
    pub fn resolve(&self) -> Result<TapeBlock, TapeBlockError> {
        if self.tapes.is_empty() {
            return Err(TapeBlockError::NoTapes);
        }
        if self.tapes.len() > usize::from(u8::MAX) {
            return Err(TapeBlockError::TooManyTapes(self.tapes.len()));
        }
        let alphabet = self
            .alphabet
            .clone()
            .or_else(|| self.tapes[0].glyphs.clone())
            .ok_or(TapeBlockError::NoAlphabet)?;
        check_glyphs(&alphabet, None)?;
        let mut tapes = Vec::with_capacity(self.tapes.len());
        for (i, t) in self.tapes.iter().enumerate() {
            let glyphs = match &t.glyphs {
                Some(own) => {
                    check_glyphs(own, Some(i))?;
                    own.clone()
                }
                None => alphabet.clone(),
            };
            if let Some(&bad) = t.cells.iter().find(|&&c| usize::from(c) >= glyphs.len()) {
                return Err(TapeBlockError::CellOutsideAlphabet {
                    tape: i,
                    index: bad,
                    width: glyphs.len(),
                });
            }
            tapes.push(Tape {
                origin: t.origin,
                cells: t.cells.clone(),
                head: t.head,
                glyphs,
            });
        }
        Ok(TapeBlock { alphabet, tapes })
    }
}

/// Bytes → block. Every tape comes back with its effective glyph table.
pub fn decode(bytes: &[u8]) -> Result<TapeBlock, TapeBlockError> {
    let file =
        TapeBlockFile::from_bytes(bytes).map_err(|e| TapeBlockError::Format(e.to_string()))?;
    let tapes = file
        .tapes
        .iter()
        .map(|t| Tape {
            origin: t.origin,
            cells: t.cells.clone(),
            head: t.head,
            glyphs: t.alphabet.clone().unwrap_or_else(|| file.alphabet.clone()),
        })
        .collect();
    Ok(TapeBlock {
        alphabet: file.alphabet,
        tapes,
    })
}

/// Block → bytes. A tape whose glyphs equal the block alphabet is written
/// as inheriting it, so the container version follows the format's own
/// rule: every tape inheriting is version 1 (what `pmt` writes), any own
/// table is version 2 (what `tmt` writes for bands that differ).
pub fn encode(block: &TapeBlockInput) -> Result<Vec<u8>, TapeBlockError> {
    let resolved = block.resolve()?;
    let file = TapeBlockFile {
        tapes: resolved
            .tapes
            .iter()
            .map(|t| TapeSnapshot {
                origin: t.origin,
                cells: t.cells.clone(),
                head: t.head,
                alphabet: (t.glyphs != resolved.alphabet).then(|| t.glyphs.clone()),
            })
            .collect(),
        alphabet: resolved.alphabet,
    };
    file.to_bytes()
        .map_err(|e| TapeBlockError::Format(e.to_string()))
}
