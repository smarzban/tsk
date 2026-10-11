// Step editing is bound to one task. Renames/removals stage; new steps save independently.
export class TaskSteps {
  constructor() {
    this.reset();
  }
  reset() {
    this.taskId = null;
    this.selected = null;
    this.editor = null;
    this.staged = null;
    this.marked = null;
    this.refusal = "";
  }
  bind(task) {
    if (this.taskId !== task.id) {
      this.reset();
      this.taskId = task.id;
    }
    task.steps ??= [];
  }
  rows(task) {
    this.bind(task);
    return this.staged ?? task.steps;
  }
  get dirty() {
    return this.staged !== null;
  }
  move(task, delta) {
    const ids = [...this.rows(task).map((step) => step.id), "add"];
    let i = ids.indexOf(this.selected);
    if (i < 0) i = delta > 0 ? -1 : 0;
    this.selected = ids[(i + delta + ids.length) % ids.length];
    this.marked = null;
  }
  begin(task, id = null) {
    this.bind(task);
    if (id) this.staged ??= structuredClone(task.steps);
    // A new step lands below the selected stored step; the selector stays on it.
    const after =
      !id && this.rows(task).some((step) => step.id === this.selected)
        ? this.selected
        : null;
    this.selected = id ?? after ?? "add";
    this.editor = {
      id,
      after,
      text: id ? this.rows(task).find((step) => step.id === id).text : "",
    };
    this.refusal = "";
    this.marked = null;
  }
  save(task, finish) {
    if (this.editor) {
      const text = this.editor.text.trim();
      if (!text) {
        this.refusal = "text required";
        return false;
      }
      if (this.editor.id) {
        this.rows(task).find((step) => step.id === this.editor.id).text = text;
        this.editor = null;
      } else {
        const step = { id: crypto.randomUUID(), text, done: false };
        const after = this.editor.after;
        const insert = (rows, value) => {
          const at = rows.findIndex((row) => row.id === after);
          rows.splice(at < 0 ? rows.length : at + 1, 0, value);
        };
        insert(task.steps, step);
        if (this.staged) insert(this.staged, structuredClone(step));
        this.selected = step.id;
        this.editor = finish
          ? null
          : { id: null, after: after ? step.id : null, text: "" };
      }
    }
    if (finish && this.staged) {
      task.steps = this.staged;
      this.staged = null;
    }
    this.refusal = "";
    return true;
  }
  cancel() {
    if (this.editor) {
      this.editor = null;
      this.refusal = "";
    } else this.staged = null;
    this.marked = null;
  }
  toggle(task) {
    const step = this.rows(task).find((step) => step.id === this.selected);
    if (!step) return;
    this.marked = null;
    step.done = !step.done;
    if (this.staged) {
      const saved = task.steps.find((row) => row.id === step.id);
      if (saved) saved.done = step.done;
    }
  }
  // alt+↑/↓: swap the selected step with its neighbour. Moves save at once, like adds.
  shift(task, delta) {
    const rows = this.rows(task);
    const from = rows.findIndex((step) => step.id === this.selected);
    const to = from + delta;
    if (from < 0 || to < 0 || to >= rows.length) return false;
    const neighbour = rows[to].id;
    [rows[from], rows[to]] = [rows[to], rows[from]];
    if (this.staged) {
      const saved = task.steps.findIndex((step) => step.id === this.selected);
      const [step] = task.steps.splice(saved, 1);
      const at = task.steps.findIndex((row) => row.id === neighbour);
      task.steps.splice(delta < 0 ? at : at + 1, 0, step);
    }
    this.marked = null;
    return true;
  }
  remove(task) {
    if (!this.rows(task).some((step) => step.id === this.selected))
      return false;
    if (this.marked !== this.selected) {
      this.marked = this.selected;
      return false;
    }
    const rows = this.rows(task);
    const index = rows.findIndex((step) => step.id === this.selected);
    rows.splice(index, 1);
    this.selected = rows[Math.min(index, rows.length - 1)]?.id ?? "add";
    this.marked = null;
    return !this.staged;
  }
}
