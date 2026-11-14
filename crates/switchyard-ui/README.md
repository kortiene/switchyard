# switchyard-ui

A read-only mission dashboard for the Switchyard Rust ledger, built with
[Rune](https://github.com/anomalyco/rune), a native GPU-rendered Rust UI
framework.

This crate fills the slot the repository README reserves as deferred `web/`:
it is the project's first graphical surface, and it is Rust, matching the
accepted direction that Rust is the sole long-term engine.

Visual language follows `.impeccable.md` at the repository root: numbered,
titled panels; a dim, padded key column against status-coloured values; status
dots and completion meters. Instrument-grade and calm until something needs
attention. Every readout is real signal from the ledger.

Rendered evidence (dark, `needs-attention` scene) lives in `artifacts/` after a
`SWITCHYARD_UI_SNAP=1` run of `scripts/ui-verify.sh`, and is regenerated rather
than trusted by hand.

## Authority

**Read-only. There is no mutation path here at all.**

- Reads go through [`switchyard_core::Host::inspect`], documented as "Read-only
  inspection. Does not take the owner lock, migrate, or dispatch." It opens the
  database `SQLITE_OPEN_READ_ONLY` with `query_only=ON`. The ledger is WAL with
  `busy_timeout(5s)`, so this coexists safely with the single writer that holds
  the flock.
- The orchestrator retains Git and forge authority. A future write path must
  shell out to the `switchyard` binary so the CLI stays the single owner, and
  must persist a `--command-id` before spawning so a lost response is
  retryable. That work is tracked separately and is deliberately absent here.
- A failed read is data, not a crash: `Ledger::error` renders as panel 1 and
  panel 2 content.

## Run

```bash
SWITCHYARD_DB=/path/to/switchyard.db cargo run -p switchyard-ui
```

`SWITCHYARD_DB` is the only configuration. `AppSpec::init` is a plain
`fn() -> S`, so it cannot capture argv; the environment works identically for a
window and for a headless run.

Toolbar: `Refresh` asks for an immediate read; `Theme` cycles Auto/Dark/Light;
`Auto` toggles the two-second poll; `Prev`/`Next` move between missions.
Previous/next become unavailable at the boundaries and carry full accessible
names. Overview also counts missions needing attention across the whole ledger,
not just the selected mission.

Before the first completed read, the dashboard says `not read` and shows
unknown—not zero—mission counts. An in-flight read says `reading` even when
polling is paused. A successful empty ledger offers `switchyard mission create`;
a failed read says `read failed`, wraps the raw error in panel 1, and offers a
recovery hint in panel 2. These are distinct states, not variations of "empty."

The selected-mission summary keeps identity, phase, and objective prominent and
folds revision/runner and base/head details into two compact rows. `3. ATTENTION`
sits immediately after it, before pipeline, limits, workspace, and command
history. Critical prose wraps rather than truncating, and capped command lists
say how many rows are not shown.

Panel 8, `CLI ECHO · INSERT, NOT EXECUTE`, is the ADW Cockpit's signature move
ported from the Pi extension: the exact `switchyard mission …` invocation for
the selected mission, as selectable text (`Cmd+C` / `Ctrl+C` copies). Reads are
badged `READ-ONLY`; anything that would change state is badged `MUTATES` with
the caveat that makes it safe to run by hand — including the `--command-id`
idempotency discipline the CLI documents. The dashboard never executes any of
it. A window 1080×780 scrolls; command history and panel 8 sit below the fold
by design, because lower-level detail and the escape hatch are drill-downs, not
ambient noise.

## Headless verification

The same binary that opens the window renders deterministic evidence without a
display, a database or a model. The clock is virtual and scenes install fixture
data with auto-refresh off, so output is byte-reproducible for a given rev.

```bash
B=crates/switchyard-ui/target/release/switchyard-ui
$B --list-scenes
$B --snap out.png --scene needs-attention --theme dark
$B --tree out.json --scene planning
$B --script scripts/approve-plan.json --frames 30 --out frames/   # when added
```

In-crate tests drive the same runtime directly, like Rune's own examples do:

```rust
let mut rt = rune::Runtime::new(ui, view::view, (1080.0, 780.0), rune::Env::new(true, 2.0));
rt.frame(0.0);
let (x, y) = rt.find("refresh").expect("the refresh button is laid out");
rt.pointer_down(x, y);
rt.pointer_up(x, y);
```

## This crate is its own workspace

`crates/switchyard-ui` is listed in the root `Cargo.toml` `exclude`. Rune pulls
wgpu, Parley, Taffy and accesskit, and needs Linux graphics system libraries at
build time, so it must not enter `scripts/rust-verify.sh`, which runs
`cargo clippy --workspace --all-targets --locked` and
`cargo test --workspace --locked` for the default feature set. The core gate
stays lean and keeps needing no graphics packages.

Use the dedicated gate:

```bash
./scripts/ui-verify.sh
```

CI runs that script as the `switchyard-ui` job in
[`.github/workflows/verify.yml`](../../.github/workflows/verify.yml). The job is
not a required check.

## Build requirements

Rust 1.99.1, pinned by the root `rust-toolchain.toml`, which matches Rune's own
toolchain exactly.

Linux build-time system libraries:

```bash
# Debian/Ubuntu
sudo apt-get install libxkbcommon-dev libwayland-dev
# windowed runs also need
sudo apt-get install libxkbcommon-x11-0 mesa-vulkan-drivers
```

Headless `--snap`, `--tree` and `--script` runs need no display and no XKB X11
library; they do need a Vulkan device, and software rendering is fine:

```bash
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json $B --snap out.png --scene planning
```

macOS and Windows need no extra system packages, but see the caveat below.

## Pinned dependency and upstream caveats

Rune is pre-release and git-only. This crate pins
`rev = "cfd4340f700f4534bfd56af4923cde925a512682"` of `dev`, which is
fast-forward-only and never rewritten. Bump the rev deliberately and re-run the
headless snapshots, since the view API moves.

Two upstream constraints to know before adopting this for real:

1. **Upstream takes no pull requests.** Bugs are filed as issues and their
   agents turn them around. There is no path to fixing a blocker locally
   quickly.
2. **Rune's workspace patches crates-io `wgpu-hal`** with `vendor/wgpu-hal`,
   and a dependency's `[patch]` does not apply to the workspace that depends on
   it. The patch only adds `dx12::Instance::expose_adapter` and lazy FXC
   loading, so Linux and macOS build unpatched (verified here on Linux aarch64).
   A **Windows** build of this crate will need the vendored `wgpu-hal` copied
   into this workspace and added as `[patch.crates-io]` before it compiles.

Build weight is real: about 40 s release on 20 cores once dependencies are
warm, and it adds the whole GPU stack to the dependency tree. That is the price
of a native UI, and the reason it lives outside the core gate.

## What is next

- The cockpit's remaining interaction moves: a command palette (the `/adw-menu`
  and `ctrl+shift+a` analogue) and drill-down overlays for mission and command
  detail, so the dashboard stays ambient and detail is summoned.
- `rune-kit` components: `Markdown` for `plan_body`, `DiffView` with
  `Annotation` for verification review and approval of the exact digest, and
  `Table` for command history. Held back from this slice deliberately so the
  dependency surface stayed minimal while the toolchain was proven.
- The write path, via the `switchyard` binary, with confirmation and a
  persisted `--command-id`.
- Publishing the headless scene snapshots from CI. The `switchyard-ui` job
  runs the gate; `SWITCHYARD_UI_SNAP=1` stays local.
