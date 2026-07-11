# Linting `.pmc` — `pmt lint`

`pmt lint` reports hygiene findings the compiler deliberately does not
warn about. It runs the compiler's analysis (through lowering, no code
generation), applies the rule catalog below, and prints one finding per
line as `FILE:LINE:COL: lint: MESSAGE`. Exit code 0 means every file is
clean; 1 means findings (or errors) somewhere. Lint reports lint
findings only — compile warnings stay on `pmt compile`.

Suppress a rule with `--allow CODE` (repeatable). Unknown codes are an
error, so a typo cannot silently disable linting.

## Project file: `pmt.json`

A repository can carry its own allow-list in a `pmt.json` file, so the
suppressions a team has agreed on travel with the source instead of
living only in shell aliases or CI flags. The schema is deliberately
tiny — today it holds nothing but the lint allow-list:

```json
{
  "lint": {
    "allow": ["unused-label", "leading-zeros"]
  }
}
```

An empty object (`{}`) is valid — an empty allow-list. Validation is
strict: any top-level key other than `lint`, any key under `lint` other
than `allow`, a non-array `allow`, a non-string entry in `allow`, or an
`allow` entry naming no rule in the catalog (below) is a hard error
naming the file and the offending key or code. A typo in a project file
must not silently do nothing, the same posture `--allow` already takes
on the command line.

`pmt lint` locates the file per input by walking up from that file's
directory through its ancestors and reading the FIRST `pmt.json` it
finds — nearest wins, and a `pmt.json` further up the tree is never
merged in, even when the nearer one exists. Two input files linted in
the same run may therefore end up governed by two different project
files, or by none. `--no-config` (`docs/cli.md`) skips this discovery
altogether, for CI invocations that want purely flag-driven behavior.

Wherever more than one source can name an allow-list for a file — the
discovered `pmt.json`, `--allow` flags on the command line, and (in an
editor session) the language server's own lint settings — the effective
list is their UNION: any one of them suppressing a code suppresses it,
and none of them can un-suppress a code another source disabled.

## Fixes

Findings may carry a machine-applicable fix, shown as an indented hint.
`--fix` applies the safe tier and rewrites the file in place; fixes that
delete or rewrite constructs on an ambiguous diagnosis are gated behind
`--fix --force` and their hints say so. After fixing, the file is linted
again — the report and the exit code describe what REMAINS. Applying a
fix can expose a new finding (deleting a redundant goto can leave its
target label unused); the re-run reports it, and repeating `--fix
--force` converges.

## Rules

### unused-label

A label is unused iff nothing in its function references it: no `goto`,
no check arm, no command successor. Labels cost zero bytes in the
binary, so this is pure source hygiene. A label on a single-statement
body is an instance of this rule, not a special case: unreferenced means
caught here; referenced means a self-loop that cannot be removed.

Fix (requires `--force`): delete the `N:` prefix. Review findings before
forcing — an unused label sometimes marks a jump you forgot to write,
and the fix removes the label, not the underlying omission.

### shadowed-import

A function definition outranks an import binding of the same bare name
in the same scope. Legal — definitions always win — but a bare `@name()`
call silently resolves to the local function while the `use` line
suggests the external. Cross-scope shadowing (inner over outer) is legal
layering and is not flagged. No fix: renaming either side is plausible.

### redundant-jump-to-next

A `goto N;` statement, or a `(N)` successor, whose target labels the
lexically next statement — fall-through is identical. Fix (requires
`--force`): delete the jump. The statement form is fixable only when the
`goto` statement carries no labels of its own (deleting a labeled
statement would orphan references to it).

### identical-check-arms

`check(N, N)`: both arms land in the same place, so the branch is
unconditional — `goto N` was meant, or one arm is a typo. `check(!, !)`
is exempt: the language has no `return` keyword, and identical-`!` arms
are its only pure mid-function return. Fix (requires `--force`,
standalone statements only): replace with `goto N`. A group-final check
is report-only — `goto` cannot appear in comma groups.

### leftover-debugger

A `debugger` statement in source. Builds strip breakpoints with
`--strip-debugger`, and an un-stripped `brk` is an optimizer
observability barrier — shipping one pessimizes `-O1` output. Fix
(requires `--force`): delete a lone, unlabeled `debugger;` statement;
labeled or comma-grouped occurrences are report-only.

### namespaced-main

A function named `main` inside a namespace is not the program entry
(only the un-namespaced top-level `main` is) and is not auto-exported —
it silently becomes an ordinary local function. No fix: rename it or
move it out.

### line-too-long

A line longer than 80 characters (character count). Report-only: where
to break a statement is layout policy, a formatter's job — `pmt fmt`
(`docs/fmt.md`) rewraps an overlong comma group by greedy-filling it
across lines. A line overlong for a different reason — a single long
command with no comma to break on, or a trailing comment that pushes an
otherwise-short line past 80 — has no break point fmt can introduce, so
it stays reported after formatting. The limit is fixed at 80.

### leading-zeros

A numeric token written with leading zeros: `007:`, `goto 007`, check
arms, call successors. Digit runs parse straight to a number, so `007`
and `7` denote the same label while looking unrelated — and `07:` next
to `7:` is a puzzling duplicate-label error. Fix (safe tier): rewrite
the token to its canonical decimal form.

### non-camel-case

Definition names the user owns — functions, namespaces, import
bindings — should be lowerCamelCase, the project's house style. The
message carries a mechanically derived rename suggestion; an import
binding's suggestion is an `as` alias. Report-only: a rename is a
multi-site edit, and renaming an exported function changes its symbol
name. The most opinionated rule in the set — `--allow non-camel-case`
is the escape hatch (note that non-ASCII identifiers, which the
language permits, do not satisfy the ASCII convention).

### confusable-names

Two definitions or bindings in the same scope whose names differ only
under a confusability normalization (case, underscores, `1`/`l`,
`i`/`l`, `0`/`o`): `sum_bits` vs `sumBits`, `fool` vs `foo1`. Reported
at the later definition, naming the earlier one. No fix.
