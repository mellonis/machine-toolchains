import * as vscode from 'vscode';
import { execFile } from 'child_process';
import {
  LanguageClient, LanguageClientOptions, ServerOptions,
} from 'vscode-languageclient/node';

// The oldest `tmt` this extension targets as its tested floor. A binary
// reporting older gets a warning, never a hard failure — the extension is
// a thin client and an older server simply answers less. Bump this in the
// same commit that raises the extension's own version whenever a newly
// required server capability lands.
const MIN_TESTED_TMT = '0.2.0';
let client: LanguageClient | undefined;

export async function activate(context: vscode.ExtensionContext) {
  const config = vscode.workspace.getConfiguration('tmt');
  const tmtPath = config.get<string>('path', 'tmt');
  checkVersion(tmtPath);

  const serverOptions: ServerOptions = { command: tmtPath, args: ['lsp'] };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ language: 'tmc' }, { language: 'tma' }],
    // Forwards the whole `tmt` section as workspace/didChangeConfiguration
    // ({ settings: { tmt: {...} } }) at startup and live on change — the
    // server unwraps the `tmt` key.
    synchronize: { configurationSection: 'tmt' },
    initializationOptions: {
      lint: {
        allow: config.get<string[]>('lint.allow', []),
        warn: config.get<string[]>('lint.warn', []),
      },
    },
  };
  client = new LanguageClient('tmt', 'tmt lsp', serverOptions, clientOptions);
  await client.start();

  const log = vscode.window.createOutputChannel('tmt');
  const provider = new TmtTaskProvider(tmtPath, log);
  // The project file is the target list's only input, so watching it is
  // a more precise invalidation than comparing mtimes on every
  // provideTasks call (docs/tmt/project.md (discovery)).
  const watcher = vscode.workspace.createFileSystemWatcher('**/tmt.json');
  watcher.onDidCreate(() => provider.invalidate());
  watcher.onDidChange(() => provider.invalidate());
  watcher.onDidDelete(() => provider.invalidate());
  context.subscriptions.push(
    log,
    watcher,
    vscode.workspace.onDidChangeWorkspaceFolders(() => provider.invalidate()),
    vscode.tasks.registerTaskProvider('tmt', provider),
  );
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}

function checkVersion(tmtPath: string) {
  execFile(tmtPath, ['--version'], (err, stdout) => {
    if (err) {
      vscode.window.showErrorMessage(
        `tmt not found at '${tmtPath}' — set tmt.path or install with ` +
        `'cargo install --path crates/turing-machine'.`);
      return;
    }
    const found = /^tmt (\d+)\.(\d+)\.(\d+)/.exec(stdout);
    if (found && older(found.slice(1).map(Number), MIN_TESTED_TMT.split('.').map(Number))) {
      vscode.window.showWarningMessage(
        `tmt ${found[1]}.${found[2]}.${found[3]} is older than the tested ` +
        `${MIN_TESTED_TMT}; some features may misbehave — update tmt.`);
    }
  });
}
function older(a: number[], b: number[]): boolean {
  for (let i = 0; i < 3; i++) { if (a[i] !== b[i]) return a[i] < b[i]; }
  return false;
}

/** One entry of `tmt build --list-targets` output. */
interface TargetEntry { name: string; run: boolean; }

/**
 * Parses `tmt build --list-targets` stdout: one line per target, the
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

class TmtTaskProvider implements vscode.TaskProvider {
  /**
   * How long a successful target list stays cached. The watcher is the
   * primary invalidation, but it can only observe files inside opened
   * workspace folders, whereas `build --list-targets` walks up from the
   * folder root without bound — so the manifest that answers may live
   * above the watched tree and never fire an event
   * (docs/tmt/project.md (discovery)). A short TTL bounds that staleness
   * without spawning a process on every `provideTasks` call.
   */
  private static readonly CACHE_TTL_MS = 5000;

  /** Target lists by workspace-folder URI, with the time each was read. */
  private cache = new Map<string, { entries: TargetEntry[]; at: number }>();

  /** Bumped by every invalidation, to detect one landing mid-fetch. */
  private epoch = 0;

  constructor(private tmtPath: string, private log: vscode.OutputChannel) {}

  /**
   * Drops every cached target list. Deliberately not per-folder: a
   * project file appearing or disappearing changes WHICH folders resolve
   * targets at all, and the cache holds at most one entry per workspace
   * folder, so a whole-cache clear costs nothing worth optimizing.
   */
  invalidate() {
    this.epoch += 1;
    this.cache.clear();
  }

  async provideTasks(): Promise<vscode.Task[]> {
    return [...this.fileTasks(), ...(await this.targetTasks())];
  }

  /** The file-scoped tasks, unchanged: they follow the active editor. */
  private fileTasks(): vscode.Task[] {
    const doc = vscode.window.activeTextEditor?.document;
    if (!doc || (doc.languageId !== 'tmc' && doc.languageId !== 'tma')) { return []; }
    const file = doc.uri.fsPath;
    const tasks = [
      this.fileTask('lint', ['lint', file], file),
      this.fileTask('fmt-check', ['fmt', '--check', file], file),
    ];
    // Each language gets its own front end: `.tmc` compiles, `.tma`
    // assembles. Both are single-file commands, so both are offered.
    if (doc.languageId === 'tmc') {
      tasks.unshift(this.fileTask('compile', ['compile', file], file));
    } else {
      tasks.unshift(this.fileTask('asm', ['asm', file], file));
    }
    return tasks;
  }

  /**
   * One `build <target>` task per declared target, plus `build --run
   * <target>` where a run block exists. The extension never looks for a
   * manifest: `tmt build --list-targets` does its own nearest-ancestor
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

  private async targetsFor(folder: vscode.WorkspaceFolder): Promise<TargetEntry[]> {
    const key = folder.uri.toString();
    const cached = this.cache.get(key);
    if (cached && Date.now() - cached.at < TmtTaskProvider.CACHE_TTL_MS) {
      return cached.entries;
    }
    const epoch = this.epoch;
    let entries: TargetEntry[];
    try {
      entries = parseTargets(await this.listTargets(folder.uri.fsPath));
    } catch (err) {
      // Failures are NOT cached. No manifest, an invalid manifest, or a
      // missing binary must cost this folder its target tasks only until
      // the next call — never for the rest of the session. The
      // file-scoped tasks are unaffected either way.
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
      execFile(this.tmtPath, ['build', '--list-targets'], { cwd }, (err, stdout, stderr) => {
        if (err) { reject(stderr.trim() || err.message); } else { resolve(stdout); }
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
    return new vscode.Task(def, vscode.TaskScope.Workspace, `tmt ${def.command}`, 'tmt',
      new vscode.ProcessExecution(this.tmtPath, args), '$tmt');
  }

  private buildTask(folder: vscode.WorkspaceFolder, target: string, run: boolean): vscode.Task {
    const def: vscode.TaskDefinition = { type: 'tmt', command: 'build', target, run };
    const args = run ? ['build', '--run', target] : ['build', target];
    const name = run ? `tmt build --run ${target}` : `tmt build ${target}`;
    return new vscode.Task(def, folder, name, 'tmt',
      new vscode.ProcessExecution(this.tmtPath, args, { cwd: folder.uri.fsPath }), '$tmt');
  }

  private fileTask(command: string, args: string[], file: string): vscode.Task {
    const def: vscode.TaskDefinition = { type: 'tmt', command, file };
    return new vscode.Task(def, vscode.TaskScope.Workspace, `tmt ${command}`, 'tmt',
      new vscode.ProcessExecution(this.tmtPath, args), '$tmt');
  }
}
