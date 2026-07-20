import * as vscode from 'vscode';
import { execFile } from 'child_process';
import {
  LanguageClient, LanguageClientOptions, ServerOptions,
} from 'vscode-languageclient/node';

// The oldest `tmt` this extension has been exercised against. A binary
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
  context.subscriptions.push(
    vscode.tasks.registerTaskProvider('tmt', new TmtTaskProvider(tmtPath)),
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

class TmtTaskProvider implements vscode.TaskProvider {
  constructor(private tmtPath: string) {}
  provideTasks(): vscode.Task[] {
    const doc = vscode.window.activeTextEditor?.document;
    if (!doc || (doc.languageId !== 'tmc' && doc.languageId !== 'tma')) { return []; }
    const file = doc.uri.fsPath;
    const tasks = [
      this.task('lint', ['lint', file], file),
      this.task('fmt-check', ['fmt', '--check', file], file),
    ];
    // Each language gets its own front end: `.tmc` compiles, `.tma`
    // assembles. Both are single-file commands, so both are offered.
    if (doc.languageId === 'tmc') {
      tasks.unshift(this.task('compile', ['compile', file], file));
    } else {
      tasks.unshift(this.task('asm', ['asm', file], file));
    }
    return tasks;
  }
  resolveTask(task: vscode.Task): vscode.Task | undefined {
    const def = task.definition as unknown as vscode.TaskDefinition & { command: string; file?: string };
    const file = def.file ?? '${file}';
    const args = def.command === 'fmt-check' ? ['fmt', '--check', file] : [def.command, file];
    return new vscode.Task(def, vscode.TaskScope.Workspace, `tmt ${def.command}`, 'tmt',
      new vscode.ProcessExecution(this.tmtPath, args), '$tmt');
  }
  private task(command: string, args: string[], file: string): vscode.Task {
    const def: vscode.TaskDefinition = { type: 'tmt', command, file };
    return new vscode.Task(def, vscode.TaskScope.Workspace, `tmt ${command}`, 'tmt',
      new vscode.ProcessExecution(this.tmtPath, args), '$tmt');
  }
}
