import * as vscode from 'vscode';
import { execFile } from 'child_process';
import {
  LanguageClient, LanguageClientOptions, ServerOptions,
} from 'vscode-languageclient/node';

const MIN_TESTED_PMT = '0.4.0';
let client: LanguageClient | undefined;

export async function activate(context: vscode.ExtensionContext) {
  const config = vscode.workspace.getConfiguration('pmt');
  const pmtPath = config.get<string>('path', 'pmt');
  checkVersion(pmtPath);

  const serverOptions: ServerOptions = { command: pmtPath, args: ['lsp'] };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ language: 'pmc' }, { language: 'pma' }],
    // Forwards the whole `pmt` section as workspace/didChangeConfiguration
    // ({ settings: { pmt: {...} } }) at startup and live on change — the
    // server unwraps the `pmt` key.
    synchronize: { configurationSection: 'pmt' },
    initializationOptions: { lint: { allow: config.get<string[]>('lint.allow', []) } },
  };
  client = new LanguageClient('pmt', 'pmt lsp', serverOptions, clientOptions);
  await client.start();

  const log = vscode.window.createOutputChannel('pmt');
  const provider = new PmtTaskProvider(pmtPath, log);
  // The project file is the target list's only input, so watching it is
  // a more precise invalidation than comparing mtimes on every
  // provideTasks call (docs/pmt/project.md (discovery)).
  const watcher = vscode.workspace.createFileSystemWatcher('**/pmt.json');
  watcher.onDidCreate(() => provider.invalidate());
  watcher.onDidChange(() => provider.invalidate());
  watcher.onDidDelete(() => provider.invalidate());
  context.subscriptions.push(
    log,
    watcher,
    vscode.workspace.onDidChangeWorkspaceFolders(() => provider.invalidate()),
    vscode.tasks.registerTaskProvider('pmt', provider),
    // Same `pmtPath` the language client and the task provider above
    // already resolved (`pmt.path`, read once at activation) — 'dap' is
    // `pmt`'s other stdio-server subcommand, alongside 'lsp'.
    vscode.debug.registerDebugAdapterDescriptorFactory('pmt', new PmtDebugAdapterDescriptorFactory(pmtPath)),
  );
}

class PmtDebugAdapterDescriptorFactory implements vscode.DebugAdapterDescriptorFactory {
  constructor(private readonly pmtPath: string) {}

  createDebugAdapterDescriptor(
    session: vscode.DebugSession,
  ): vscode.ProviderResult<vscode.DebugAdapterDescriptor> {
    // Run the adapter with the workspace folder as its cwd so relative
    // `program`/`tape` paths in launch.json resolve against the project,
    // not against whatever directory VS Code itself was launched from.
    const cwd =
      session.workspaceFolder?.uri.fsPath ??
      vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    return new vscode.DebugAdapterExecutable(this.pmtPath, ['dap'], cwd ? { cwd } : undefined);
  }
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}

function checkVersion(pmtPath: string) {
  execFile(pmtPath, ['--version'], (err, stdout) => {
    if (err) {
      vscode.window.showErrorMessage(
        `pmt not found at '${pmtPath}' — set pmt.path or install with ` +
        `'cargo install --path crates/post-machine'.`);
      return;
    }
    const found = /^pmt (\d+)\.(\d+)\.(\d+)/.exec(stdout);
    if (found && older(found.slice(1).map(Number), MIN_TESTED_PMT.split('.').map(Number))) {
      vscode.window.showWarningMessage(
        `pmt ${found[1]}.${found[2]}.${found[3]} is older than the tested ` +
        `${MIN_TESTED_PMT}; some features may misbehave — update pmt.`);
    }
  });
}
function older(a: number[], b: number[]): boolean {
  for (let i = 0; i < 3; i++) { if (a[i] !== b[i]) return a[i] < b[i]; }
  return false;
}

/** One entry of `pmt build --list-targets` output. */
interface TargetEntry { name: string; run: boolean; }

/**
 * Parses `pmt build --list-targets` stdout: one line per target, the
 * name optionally followed by a TAB and the literal `run` when the
 * target declares a run block. The format is pinned by the crate's
 * build_driver tests.
 */
function parseTargets(stdout: string): TargetEntry[] {
  return stdout
    .split('\n')
    .filter((line) => line.length > 0)
    .map((line) => {
      const [name, marker] = line.split('\t');
      return { name, run: marker === 'run' };
    })
    .filter((entry) => entry.name.length > 0);
}

class PmtTaskProvider implements vscode.TaskProvider {
  /**
   * How long a successful target list stays cached. The watcher is the
   * primary invalidation, but it can only observe files inside opened
   * workspace folders, whereas `build --list-targets` walks up from the
   * folder root without bound — so the manifest that answers may live
   * above the watched tree and never fire an event
   * (docs/pmt/project.md (discovery)). A short TTL bounds that staleness
   * without spawning a process on every `provideTasks` call.
   */
  private static readonly CACHE_TTL_MS = 5000;

  /** Target lists by workspace-folder URI, with the time each was read. */
  private cache = new Map<string, { entries: TargetEntry[]; at: number }>();

  /** Bumped by every invalidation, to detect one landing mid-fetch. */
  private epoch = 0;

  /**
   * Lookups currently awaiting the binary, by folder URI. `provideTasks`
   * is called often enough that two calls can overlap; the second joins
   * the first's promise instead of spawning a second process.
   */
  private inFlight = new Map<string, Promise<TargetEntry[]>>();

  constructor(private pmtPath: string, private log: vscode.OutputChannel) {}

  /**
   * Drops every cached target list. Deliberately not per-folder: a
   * project file appearing or disappearing changes WHICH folders resolve
   * targets at all, and the cache holds at most one entry per workspace
   * folder, so a whole-cache clear costs nothing worth optimizing.
   *
   * In-flight lookups are dropped too, not just cached results: a fetch
   * that started before the edit would otherwise hand pre-edit data to
   * every caller that joined it. The abandoned fetch still completes, but
   * its write-back is refused by the epoch check.
   */
  invalidate() {
    this.epoch += 1;
    this.cache.clear();
    this.inFlight.clear();
  }

  async provideTasks(): Promise<vscode.Task[]> {
    return [...this.fileTasks(), ...(await this.targetTasks())];
  }

  /** The file-scoped tasks, unchanged: they follow the active editor. */
  private fileTasks(): vscode.Task[] {
    const doc = vscode.window.activeTextEditor?.document;
    if (!doc || (doc.languageId !== 'pmc' && doc.languageId !== 'pma')) { return []; }
    const file = doc.uri.fsPath;
    const tasks = [
      this.fileTask('lint', ['lint', file], file),
      this.fileTask('fmt-check', ['fmt', '--check', file], file),
    ];
    // `compile` stays .pmc-only — a .pma file assembles via `pmt asm`,
    // which this task provider doesn't offer (see the README).
    if (doc.languageId === 'pmc') {
      tasks.unshift(this.fileTask('compile', ['compile', file], file));
    }
    return tasks;
  }

  /**
   * One `build <target>` task per declared target, plus `build --run
   * <target>` where a run block exists. The extension never looks for a
   * manifest: `pmt build --list-targets` does its own nearest-ancestor
   * discovery from its working directory, so running it at the folder
   * root delegates the whole walk to the binary.
   */
  private async targetTasks(): Promise<vscode.Task[]> {
    const folders = vscode.workspace.workspaceFolders ?? [];
    const out: vscode.Task[] = [];
    for (const folder of folders) {
      for (const entry of await this.targetsFor(folder)) {
        out.push(this.buildTask(folder, entry.name, false));
        if (entry.run) { out.push(this.buildTask(folder, entry.name, true)); }
      }
    }
    return out;
  }

  /**
   * This folder's targets — from the cache while fresh, otherwise from
   * the binary. Every return is a COPY: the cached array is this
   * provider's own state, and handing it out would let any caller's
   * mutation corrupt what later calls read.
   */
  private async targetsFor(folder: vscode.WorkspaceFolder): Promise<TargetEntry[]> {
    const key = folder.uri.toString();
    const cached = this.cache.get(key);
    if (cached && Date.now() - cached.at < PmtTaskProvider.CACHE_TTL_MS) {
      return [...cached.entries];
    }
    const running = this.inFlight.get(key);
    if (running) { return [...(await running)]; }
    const fetch = this.fetchTargets(folder, key);
    this.inFlight.set(key, fetch);
    try {
      return [...(await fetch)];
    } finally {
      // Only retire our own entry: an invalidation during the fetch clears
      // the map, and a later call may already have registered a new one.
      if (this.inFlight.get(key) === fetch) { this.inFlight.delete(key); }
    }
  }

  /**
   * Runs the binary and records the result. RESOLVES, never rejects —
   * every failure becomes an empty list plus a log line. That is what
   * makes the promise safe to share between concurrent callers: a
   * rejection here would propagate to all of them and surface as
   * `provideTasks` throwing, costing the user the file-scoped tasks too.
   */
  private async fetchTargets(
    folder: vscode.WorkspaceFolder,
    key: string,
  ): Promise<TargetEntry[]> {
    const epoch = this.epoch;
    let entries: TargetEntry[];
    try {
      entries = parseTargets(await this.listTargets(folder.uri.fsPath));
    } catch (err) {
      // Failures are NOT cached. No manifest, an invalid manifest, or a
      // missing binary must cost this folder its target tasks only until
      // the next call — never for the rest of the session.
      this.log.appendLine(`[${folder.name}] build --list-targets: ${err}`);
      return [];
    }
    // Drop the write-back if an invalidation landed while awaiting, or
    // this completing fetch would restore pre-edit data.
    if (epoch === this.epoch) {
      this.cache.set(key, { entries, at: Date.now() });
    }
    return entries;
  }

  private listTargets(cwd: string): Promise<string> {
    return new Promise((resolve, reject) => {
      execFile(this.pmtPath, ['build', '--list-targets'], { cwd }, (err, stdout, stderr) => {
        if (!err) { resolve(stdout); return; }
        // The log line is the only diagnostic a user gets when a folder
        // quietly contributes no target tasks, so keep the exit status
        // alongside the binary's own message. `code` is the exit code for
        // a process that ran, or a string like `ENOENT` when the spawn
        // itself failed — both worth naming.
        const detail = stderr.trim() || err.message;
        const { code } = err as Error & { code?: number | string };
        reject(new Error(code === undefined ? detail : `exit ${code}: ${detail}`));
      });
    });
  }

  resolveTask(task: vscode.Task): vscode.Task | undefined {
    const def = task.definition as unknown as vscode.TaskDefinition & {
      command: string; file?: string; target?: string; run?: boolean;
    };
    if (def.command === 'build') {
      // A per-target task MUST know its folder: `--list-targets`
      // discovery is cwd-driven, so resolving with the wrong cwd would
      // not fail — it would silently build a different project's target
      // of the same name. Refuse rather than guess.
      const scope = task.scope;
      if (!scope || typeof scope === 'number' || !def.target) { return undefined; }
      return this.buildTask(scope, def.target, def.run === true);
    }
    const file = def.file ?? '${file}';
    const args = def.command === 'fmt-check' ? ['fmt', '--check', file] : [def.command, file];
    return new vscode.Task(def, vscode.TaskScope.Workspace, `pmt ${def.command}`, 'pmt',
      new vscode.ProcessExecution(this.pmtPath, args), '$pmt');
  }

  private buildTask(folder: vscode.WorkspaceFolder, target: string, run: boolean): vscode.Task {
    const def: vscode.TaskDefinition = { type: 'pmt', command: 'build', target, run };
    const args = run ? ['build', '--run', target] : ['build', target];
    const name = run ? `pmt build --run ${target}` : `pmt build ${target}`;
    return new vscode.Task(def, folder, name, 'pmt',
      new vscode.ProcessExecution(this.pmtPath, args, { cwd: folder.uri.fsPath }), '$pmt');
  }

  private fileTask(command: string, args: string[], file: string): vscode.Task {
    const def: vscode.TaskDefinition = { type: 'pmt', command, file };
    return new vscode.Task(def, vscode.TaskScope.Workspace, `pmt ${command}`, 'pmt',
      new vscode.ProcessExecution(this.pmtPath, args), '$pmt');
  }
}
