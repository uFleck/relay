import {
  errorText,
  onV2Change,
  v2WorkspaceSnapshot,
  type V2WorkspaceSnapshot,
} from "./ipc";

export interface WorkspaceState {
  workspace: V2WorkspaceSnapshot | null;
  version: number;
  error: string | null;
}

const INITIAL: WorkspaceState = {
  workspace: null,
  version: 0,
  error: null,
};

type Listener = () => void;
type Load = () => Promise<V2WorkspaceSnapshot>;
type Subscribe = (listener: Listener) => () => void;

export class WorkspaceStore {
  private state: WorkspaceState = INITIAL;
  private listeners = new Set<Listener>();
  private sequence = 0;
  private started = false;
  private scheduled = false;

  constructor(
    private load: Load = v2WorkspaceSnapshot,
    private subscribeToChanges: Subscribe = onV2Change,
  ) {}

  getSnapshot = (): WorkspaceState => this.state;

  subscribe = (listener: Listener): (() => void) => {
    this.start();
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  refresh = async (): Promise<void> => {
    const request = ++this.sequence;
    try {
      const workspace = await this.load();
      if (request !== this.sequence) return;
      this.state = {
        workspace,
        version: this.state.version + 1,
        error: null,
      };
      this.emit();
    } catch (cause) {
      if (request !== this.sequence) return;
      this.state = { ...this.state, error: errorText(cause) };
      this.emit();
    }
  };

  private start() {
    if (this.started) return;
    this.started = true;
    this.subscribeToChanges(() => this.scheduleRefresh());
    void this.refresh();
  }

  private scheduleRefresh() {
    if (this.scheduled) return;
    this.scheduled = true;
    queueMicrotask(() => {
      this.scheduled = false;
      void this.refresh();
    });
  }

  private emit() {
    for (const listener of this.listeners) listener();
  }
}

export const workspaceStore = new WorkspaceStore();
