import * as vscode from 'vscode';
import { execFile } from 'child_process';
import {
  LanguageClient, LanguageClientOptions, ServerOptions,
} from 'vscode-languageclient/node';

const MIN_TESTED_PMT = '0.1.0';
let client: LanguageClient | undefined;

export async function activate(context: vscode.ExtensionContext) {
  const config = vscode.workspace.getConfiguration('pmt');
  const pmtPath = config.get<string>('path', 'pmt');
  checkVersion(pmtPath);

  const serverOptions: ServerOptions = { command: pmtPath, args: ['lsp'] };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ language: 'pmc' }],
    // Forwards the whole `pmt` section as workspace/didChangeConfiguration
    // ({ settings: { pmt: {...} } }) at startup and live on change — the
    // server unwraps the `pmt` key.
    synchronize: { configurationSection: 'pmt' },
    initializationOptions: { lint: { allow: config.get<string[]>('lint.allow', []) } },
  };
  client = new LanguageClient('pmt', 'pmt lsp', serverOptions, clientOptions);
  await client.start();
  context.subscriptions.push(
    vscode.tasks.registerTaskProvider('pmt', new PmtTaskProvider(pmtPath)),
  );
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

class PmtTaskProvider implements vscode.TaskProvider {
  constructor(private pmtPath: string) {}
  provideTasks(): vscode.Task[] {
    const doc = vscode.window.activeTextEditor?.document;
    if (!doc || doc.languageId !== 'pmc') { return []; }
    const file = doc.uri.fsPath;
    return [
      this.task('compile', ['compile', file], file),
      this.task('lint', ['lint', file], file),
      this.task('fmt-check', ['fmt', '--check', file], file),
    ];
  }
  resolveTask(task: vscode.Task): vscode.Task | undefined {
    const def = task.definition as unknown as vscode.TaskDefinition & { command: string; file?: string };
    const file = def.file ?? '${file}';
    const args = def.command === 'fmt-check' ? ['fmt', '--check', file] : [def.command, file];
    return new vscode.Task(def, vscode.TaskScope.Workspace, `pmt ${def.command}`, 'pmt',
      new vscode.ProcessExecution(this.pmtPath, args), '$pmt');
  }
  private task(command: string, args: string[], file: string): vscode.Task {
    const def: vscode.TaskDefinition = { type: 'pmt', command, file };
    return new vscode.Task(def, vscode.TaskScope.Workspace, `pmt ${command}`, 'pmt',
      new vscode.ProcessExecution(this.pmtPath, args), '$pmt');
  }
}
