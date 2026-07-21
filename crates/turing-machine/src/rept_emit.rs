//! Re-detection of arithmetic families in stamped assembly, rewriting them
//! back into `.rept` loops (docs/formats.md (the .rept macro)).
//!
//! Codegen emits STAMPED `.tma`: a range-expanded family becomes one labeled
//! block (or one match-table row) per value, so a 127-way increment is 127
//! near-identical blocks. This pass reads that text and, where a maximal run
//! of at least four consecutive blocks (or same-label table rows) tokenizes
//! identically except for integers at fixed positions, folds the run into a
//! single `.rept v, lo, hi` … `.endr` with `{…}` substitution expressions —
//! the compact form a human would have written.
//!
//! The pass is safe by construction: it changes only how the text READS,
//! never what it ASSEMBLES. After rewriting, it assembles BOTH the stamped
//! input and the compressed output through the TM-1 dialect and compares the
//! object bytes; on any mismatch — or any error assembling the compressed
//! side — it returns the stamped text untouched. Detection can therefore be
//! optimistic: a mis-inferred family simply fails the byte compare and falls
//! back. When nothing is rewritten the input is returned verbatim with no
//! assembly at all.
//!
//! Substitution grammar constraints (docs/formats.md (the .rept macro)): the
//! assembler rejects a negative `%` remainder, so every emitted expression is
//! built to stay non-negative — a modular step of `-1` is written as `+N-1`,
//! which is exactly what the modular inference produces (the offset is the
//! first value, always in `0..N`). The loop variable `v` cannot collide with
//! any label or mnemonic: substitution only rewrites `{…}` occurrences, and
//! the surrounding stamped text carries no braces.

use mtc_core::asm::ArchSyntax;
use mtc_core::formats::ARCH_TM1;
use mtc_core::formats::object::ObjectFile;

/// The loop-variable name used in every emitted `.rept` header and `{…}`
/// expression. Matches the flagship hand-written UTM (docs/formats.md (the
/// .rept macro)).
const VAR: &str = "v";

/// Minimum members for a run to be worth folding.
const MIN_RUN: usize = 4;

/// What `compress_asm` did — informational, rendered by the CLI under `-v`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReptEmitReport {
    /// How many `.rept` runs the compressed output carries.
    pub runs_compressed: usize,
    /// Physical line count of the stamped input.
    pub lines_before: usize,
    /// Physical line count of the returned text (== `lines_before` when the
    /// pass made no change or fell back).
    pub lines_after: usize,
    /// The self-check failed (byte mismatch or a compressed-side assemble
    /// error), so the stamped text was returned unchanged.
    pub fell_back: bool,
}

/// Compress stamped assembly text by rewriting arithmetic families as `.rept`.
/// `syntax` is the TM-1 dialect used for the always-on self-check. Returns the
/// text to use as the `-S` artifact plus a report; on any self-check failure
/// the stamped input is returned with `fell_back: true`.
pub fn compress_asm(text: &str, syntax: &ArchSyntax) -> (String, ReptEmitReport) {
    let (out, report, _obj) = compress_asm_with_object(text, syntax);
    (out, report)
}

/// The pipeline entry: like [`compress_asm`], but also hands back the winning
/// object assembled during the self-check so the caller need not assemble a
/// third time. `Some(object)` whenever a self-check ran (the compressed object
/// when it stuck, the stamped object when it fell back); `None` when nothing
/// was rewritten (no assembly happened) or the STAMPED side itself failed to
/// assemble — in both cases the caller assembles the returned text itself,
/// which is byte-for-byte today's behaviour (and surfaces a stamped-side
/// failure as the usual internal error). The self-check always assembles
/// without debug info: the two sides carry different physical lines, so their
/// debug sections would never match — the code image is what must agree.
pub(crate) fn compress_asm_with_object(
    text: &str,
    syntax: &ArchSyntax,
) -> (String, ReptEmitReport, Option<ObjectFile>) {
    let lines_before = line_count(text);
    let (candidate, runs) = rewrite(text);
    if runs == 0 {
        // Nothing changed — return the input verbatim, no assembly.
        return (
            text.to_string(),
            ReptEmitReport {
                runs_compressed: 0,
                lines_before,
                lines_after: lines_before,
                fell_back: false,
            },
            None,
        );
    }

    // Self-check: the stamped and compressed sides must assemble to the same
    // object bytes. A stamped-side failure is a real codegen bug — signal it
    // by returning `None` so the caller re-assembles the stamped text and
    // reports the internal error through its normal path.
    let Ok(stamped_obj) = assemble_self_check(syntax, text) else {
        return (
            text.to_string(),
            ReptEmitReport {
                runs_compressed: 0,
                lines_before,
                lines_after: lines_before,
                fell_back: true,
            },
            None,
        );
    };
    match assemble_self_check(syntax, &candidate) {
        Ok(compressed_obj) if compressed_obj.to_bytes() == stamped_obj.to_bytes() => (
            candidate.clone(),
            ReptEmitReport {
                runs_compressed: runs,
                lines_before,
                lines_after: line_count(&candidate),
                fell_back: false,
            },
            Some(compressed_obj),
        ),
        _ => (
            text.to_string(),
            ReptEmitReport {
                runs_compressed: 0,
                lines_before,
                lines_after: lines_before,
                fell_back: true,
            },
            Some(stamped_obj),
        ),
    }
}

fn assemble_self_check(syntax: &ArchSyntax, text: &str) -> Result<ObjectFile, ()> {
    mtc_core::asm::assemble(syntax, ARCH_TM1, text, false).map_err(|_| ())
}

fn line_count(text: &str) -> usize {
    text.split('\n').count()
}

// ---------------------------------------------------------------------------
// Text rewriting (pure — no assembly). Splitting on '\n' and re-joining with
// '\n' is the identity on any text, so the no-run path is byte-preserving.
// ---------------------------------------------------------------------------

/// Rewrite every fully-inferable family in `text` into a `.rept`, returning the
/// new text and the number of runs folded. Everything else passes through
/// verbatim.
fn rewrite(text: &str) -> (String, usize) {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut runs = 0;
    let mut i = 0;
    while i < lines.len() {
        if let Some((end, rept)) = try_table_row_run(&lines, i) {
            out.extend(rept);
            runs += 1;
            i = end;
        } else if let Some((end, rept)) = try_code_block_run(&lines, i) {
            out.extend(rept);
            runs += 1;
            i = end;
        } else {
            out.push(lines[i].to_string());
            i += 1;
        }
    }
    (out.join("\n"), runs)
}

// ---------------------------------------------------------------------------
// Tokenization: an integer hole is a maximal decimal run that is NOT glued to
// a following identifier character. This captures standalone operands AND an
// identifier's trailing decimal (`plus__88` → prefix `plus__` + `88`), while a
// non-trailing digit run inside an identifier (`plu5s`) bails the whole line.
// ---------------------------------------------------------------------------

/// One integer hole in a line: its byte range and value.
#[derive(Debug, Clone)]
struct Hole {
    start: usize,
    end: usize,
    value: i64,
}

/// Split `s` into a template (each hole replaced by `\0`) plus the ordered
/// holes. `None` when a decimal run is immediately followed by an identifier
/// character (a non-trailing digit inside an identifier — not parameterizable)
/// or a run overflows `i64`.
fn tokenize_holes(s: &str) -> Option<(String, Vec<Hole>)> {
    let mut template = String::new();
    let mut holes = Vec::new();
    let mut chars = s.char_indices().peekable();
    while let Some((idx, c)) = chars.next() {
        if c.is_ascii_digit() {
            let start = idx;
            let mut end = idx + c.len_utf8();
            while let Some(&(j, d)) = chars.peek() {
                if d.is_ascii_digit() {
                    end = j + d.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            // A letter or `_` right after the run means the digits are part of
            // an identifier and not a trailing decimal — bail conservatively.
            if chars
                .peek()
                .is_some_and(|&(_, d)| d.is_alphabetic() || d == '_')
            {
                return None;
            }
            let value: i64 = s[start..end].parse().ok()?;
            template.push('\0');
            holes.push(Hole { start, end, value });
        } else {
            template.push(c);
        }
    }
    Some((template, holes))
}

/// Replace each `(start, end, replacement)` edit in `line`, applied right to
/// left so earlier byte offsets stay valid.
fn splice(line: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|e| e.0);
    let mut result = line.to_string();
    for (start, end, repl) in edits.into_iter().rev() {
        result.replace_range(start..end, &repl);
    }
    result
}

// ---------------------------------------------------------------------------
// Progression inference over a hole's values across a run's members.
// ---------------------------------------------------------------------------

enum Progression {
    /// Same value at every member — emit the literal (leave member 0's text).
    Constant,
    /// `value_i == first + i` — emit `{v}` or `{v+first}`.
    Affine(i64),
    /// `value_i == (first + i) % n` with at least one wrap — emit `{(v+first)%n}`.
    Modular { first: i64, n: i64 },
}

/// Classify a hole's values (length ≥ 1). `None` when no supported progression
/// fits — the run is then left stamped.
fn infer(values: &[i64]) -> Option<Progression> {
    let first = values[0];
    if values.iter().all(|&v| v == first) {
        return Some(Progression::Constant);
    }
    // Every value the assembler sees here is a symbol index or a label suffix
    // — non-negative — so a negative first value is not a family we model.
    if first < 0 {
        return None;
    }
    // All arithmetic is checked: values come straight from the source and can
    // sit near `i64::MAX`, so an overflow here would be a detection-time panic
    // BEFORE the self-check could catch it. Overflow → no progression fits →
    // the run stays stamped, matching the conservative framing.
    // Affine: strictly `first + i`, no wrap.
    if values
        .iter()
        .enumerate()
        .all(|(i, &v)| first.checked_add(i as i64) == Some(v))
    {
        return Some(Progression::Affine(first));
    }
    // Modular: `(first + i) % n` with `n = max + 1` and at least one wrap. The
    // step is `+1`, so `first + i` is always non-negative and the emitted `%`
    // never yields a negative remainder.
    let n = values.iter().max().copied()?.checked_add(1)?;
    if n >= 1
        && values
            .iter()
            .enumerate()
            .all(|(i, &v)| first.checked_add(i as i64).is_some_and(|raw| raw % n == v))
        && values
            .iter()
            .enumerate()
            .any(|(i, _)| first.checked_add(i as i64).is_some_and(|raw| raw >= n))
    {
        return Some(Progression::Modular { first, n });
    }
    None
}

/// The `{…}` expression for a progression, or `None` for a constant (whose
/// literal is left in place unchanged).
fn expr(p: &Progression) -> Option<String> {
    match p {
        Progression::Constant => None,
        Progression::Affine(0) => Some(format!("{{{VAR}}}")),
        Progression::Affine(f) => Some(format!("{{{VAR}+{f}}}")),
        Progression::Modular { first: 0, n } => Some(format!("{{{VAR}%{n}}}")),
        Progression::Modular { first, n } => Some(format!("{{({VAR}+{first})%{n}}}")),
    }
}

/// Infer each hole of a run, given its values grouped by hole (`per_hole[k]`
/// is hole `k`'s value at every member). Returns one expression slot per hole
/// — `Some(expr)` for a varying hole, `None` for a constant. `None` for the
/// whole run when ANY hole has no supported progression (the run then stays
/// stamped).
fn run_exprs(per_hole: &[Vec<Hole>]) -> Option<Vec<Option<String>>> {
    let mut out = Vec::with_capacity(per_hole.len());
    for holes in per_hole {
        let values: Vec<i64> = holes.iter().map(|h| h.value).collect();
        out.push(expr(&infer(&values)?));
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Line classification.
// ---------------------------------------------------------------------------

/// The byte length of a leading `<ident>:` label in `s` (including the colon),
/// or `None`. Identifiers are ASCII `[A-Za-z_][A-Za-z0-9_]*`.
fn label_prefix(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let first = *bytes.first()?;
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return None;
    }
    let mut i = 1;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    (bytes.get(i) == Some(&b':')).then_some(i + 1)
}

/// A code-block label line: `<ident>:` at column 0 with nothing after but
/// optional whitespace or a comment — exactly what codegen emits for a printed
/// block label. Table heads (`T0: .row …`) carry a directive after the colon
/// and are excluded.
fn is_code_label(line: &str) -> bool {
    if line.starts_with(char::is_whitespace) {
        return false;
    }
    match label_prefix(line) {
        Some(len) => {
            let rest = line[len..].trim_start();
            rest.is_empty() || rest.starts_with(';')
        }
        None => false,
    }
}

/// A code-block body line: indented, non-blank, and not an (indented) directive
/// — the instruction lines under a label.
fn is_body_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    !trimmed.is_empty() && line.starts_with(char::is_whitespace) && !trimmed.starts_with('.')
}

/// A parsed match/dispatch table row: whether it carries a label, its directive
/// (`row` / `target`, never the bulk `targets`), and the byte offset where its
/// operand begins.
struct RowInfo {
    labeled: bool,
    directive: &'static str,
    operand_start: usize,
}

/// Classify a table-row line (`[<label>:] .row|.target <operand>`), or `None`.
fn classify_row(line: &str) -> Option<RowInfo> {
    let lead = line.len() - line.trim_start().len();
    let after_ws = &line[lead..];
    let (labeled, after_label) = match label_prefix(after_ws) {
        Some(len) => (true, lead + len),
        None => (false, lead),
    };
    let dir_area = line[after_label..].trim_start();
    let dir_start = line.len() - dir_area.len();
    let directive = if starts_with_token(dir_area, ".row") {
        "row"
    } else if starts_with_token(dir_area, ".target") {
        "target"
    } else {
        return None;
    };
    let after_dir = &line[dir_start + directive.len() + 1..];
    let operand_start = line.len() - after_dir.trim_start().len();
    Some(RowInfo {
        labeled,
        directive,
        operand_start,
    })
}

/// `s` begins with `token` followed by a token boundary (whitespace or end) —
/// so `.row` matches `.row [..]` but not `.rowx`, and `.target` does not match
/// the bulk `.targets`.
fn starts_with_token(s: &str, token: &str) -> bool {
    s.strip_prefix(token)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

// ---------------------------------------------------------------------------
// Code-block runs: a labeled block (label line + indented body) repeated with
// integers varying at fixed positions. The label's own trailing decimal is a
// varying hole, so `L0`/`L1`/… collapse to `L{v}`.
// ---------------------------------------------------------------------------

/// A parsed code block: its line span `[start, end)`, its template (per-line
/// templates joined by `\n`), and its holes tagged with their line offset.
struct Block {
    end: usize,
    template: String,
    /// One entry per body line (index 0 = the label line): the holes on it.
    line_holes: Vec<Vec<Hole>>,
}

/// Parse the code block starting at `lines[start]`, or `None` when `lines[start]`
/// is not a code label or any line is not tokenizable.
fn parse_block(lines: &[&str], start: usize) -> Option<Block> {
    if !is_code_label(lines[start]) {
        return None;
    }
    let mut end = start + 1;
    while end < lines.len() && is_body_line(lines[end]) {
        end += 1;
    }
    let mut template = String::new();
    let mut line_holes = Vec::with_capacity(end - start);
    for (off, &line) in lines[start..end].iter().enumerate() {
        let (tmpl, holes) = tokenize_holes(line)?;
        if off > 0 {
            template.push('\n');
        }
        template.push_str(&tmpl);
        line_holes.push(holes);
    }
    Some(Block {
        end,
        template,
        line_holes,
    })
}

/// If `lines[i]` starts a maximal run of ≥ [`MIN_RUN`] identically-templated
/// code blocks whose every varying hole has a supported progression, return the
/// end line index and the `.rept` replacement lines.
fn try_code_block_run(lines: &[&str], i: usize) -> Option<(usize, Vec<String>)> {
    let first = parse_block(lines, i)?;
    let mut members = vec![first];
    loop {
        let pos = members.last().unwrap().end;
        match parse_block(lines, pos) {
            Some(b) if b.template == members[0].template => members.push(b),
            _ => break,
        }
    }
    if members.len() < MIN_RUN {
        return None;
    }
    let end = members.last().unwrap().end;
    let line_count = members[0].line_holes.len();

    // Per hole, gather values across members in the block's hole order.
    let hole_count: usize = members[0].line_holes.iter().map(Vec::len).sum();
    let mut per_hole: Vec<Vec<Hole>> = vec![Vec::with_capacity(members.len()); hole_count];
    for m in &members {
        let mut k = 0;
        for holes in &m.line_holes {
            for h in holes {
                per_hole[k].push(h.clone());
                k += 1;
            }
        }
    }
    let exprs = run_exprs(&per_hole)?;

    // Emit member 0's lines with each varying hole spliced.
    let mut rept = Vec::with_capacity(line_count + 2);
    rept.push(format!(".rept {VAR}, 0, {}", members.len() - 1));
    let mut k = 0;
    for off in 0..line_count {
        let raw = lines[i + off];
        let mut edits = Vec::new();
        for h in &members[0].line_holes[off] {
            if let Some(e) = &exprs[k] {
                edits.push((h.start, h.end, e.clone()));
            }
            k += 1;
        }
        rept.push(splice(raw, edits));
    }
    rept.push(".endr".to_string());
    Some((end, rept))
}

// ---------------------------------------------------------------------------
// Table-row runs: a labeled `.row`/`.target` head plus its unlabeled
// continuation rows (`emit_table`'s shape). Only the operand varies; the label
// is constant and re-emitted every iteration so the rows continue one table.
// ---------------------------------------------------------------------------

/// If `lines[i]` is a labeled `.row`/`.target` head followed by ≥ [`MIN_RUN`]-1
/// unlabeled continuation rows sharing an operand template with a supported
/// varying progression, return the end line index and the `.rept` replacement.
fn try_table_row_run(lines: &[&str], i: usize) -> Option<(usize, Vec<String>)> {
    let head = classify_row(lines[i])?;
    if !head.labeled {
        return None;
    }
    // Gather the head plus contiguous unlabeled rows of the same directive.
    let mut end = i + 1;
    while end < lines.len() {
        match classify_row(lines[end]) {
            Some(r) if !r.labeled && r.directive == head.directive => end += 1,
            _ => break,
        }
    }
    let members = i..end;
    if members.len() < MIN_RUN {
        return None;
    }

    // Compare operand templates and gather operand holes per member. Offsets
    // are relative to each member's operand substring; only member 0's matter
    // for emission.
    let mut template: Option<String> = None;
    let mut per_hole: Vec<Vec<Hole>> = Vec::new();
    let mut head_holes: Vec<Hole> = Vec::new();
    for (idx, line_no) in members.clone().enumerate() {
        let info = classify_row(lines[line_no])?;
        let operand = &lines[line_no][info.operand_start..];
        let (tmpl, holes) = tokenize_holes(operand)?;
        match &template {
            None => {
                template = Some(tmpl);
                per_hole = vec![Vec::with_capacity(members.len()); holes.len()];
            }
            Some(t) if *t == tmpl => {}
            Some(_) => return None,
        }
        for (k, h) in holes.iter().enumerate() {
            per_hole[k].push(h.clone());
        }
        if idx == 0 {
            head_holes = holes;
        }
    }
    if per_hole.is_empty() {
        return None; // no varying operand — nothing to fold
    }
    let exprs = run_exprs(&per_hole)?;

    // Emit member 0's full line (label + directive kept) with operand holes
    // spliced at their absolute offsets.
    let head_line = lines[i];
    let base = classify_row(head_line)?.operand_start;
    let mut edits = Vec::new();
    for (k, h) in head_holes.iter().enumerate() {
        if let Some(e) = &exprs[k] {
            edits.push((base + h.start, base + h.end, e.clone()));
        }
    }
    let rept = vec![
        format!(".rept {VAR}, 0, {}", members.len() - 1),
        splice(head_line, edits),
        ".endr".to_string(),
    ];
    Some((end, rept))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_splits_trailing_decimal_and_standalone() {
        let (t, h) = tokenize_holes("Linc88:   wr [3]").unwrap();
        assert_eq!(t, "Linc\0:   wr [\0]");
        assert_eq!(h.iter().map(|h| h.value).collect::<Vec<_>>(), vec![88, 3]);
    }

    #[test]
    fn tokenize_bails_on_interior_digits() {
        // A digit run glued to a following identifier char is not a trailing
        // decimal and cannot be parameterized.
        assert!(tokenize_holes("plu5s").is_none());
        assert!(tokenize_holes("0x10").is_none());
    }

    #[test]
    fn infer_recognizes_the_three_shapes() {
        assert!(matches!(infer(&[5, 5, 5, 5]), Some(Progression::Constant)));
        assert!(matches!(infer(&[3, 4, 5, 6]), Some(Progression::Affine(3))));
        // (v+1)%6 over 0..5 → 1,2,3,4,5,0
        assert!(matches!(
            infer(&[1, 2, 3, 4, 5, 0]),
            Some(Progression::Modular { first: 1, n: 6 })
        ));
        // No wrap, not strictly affine → unsupported.
        assert!(infer(&[0, 2, 4, 6]).is_none());
    }

    #[test]
    fn expr_forms_are_assembler_legal() {
        assert_eq!(expr(&Progression::Affine(0)).unwrap(), "{v}");
        assert_eq!(expr(&Progression::Affine(3)).unwrap(), "{v+3}");
        assert_eq!(
            expr(&Progression::Modular { first: 1, n: 6 }).unwrap(),
            "{(v+1)%6}"
        );
        assert_eq!(
            expr(&Progression::Modular { first: 0, n: 4 }).unwrap(),
            "{v%4}"
        );
        assert!(expr(&Progression::Constant).is_none());
    }

    #[test]
    fn classify_row_excludes_bulk_targets_and_code_labels() {
        assert!(classify_row("T0:     .row    [0]").is_some());
        assert!(classify_row("        .row    [1]").is_some());
        assert!(classify_row("D0:     .targets a, b").is_none());
        assert!(classify_row("L0:").is_none());
    }
}
