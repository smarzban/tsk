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
    // The task page's PAPER TRAIL is expanded (`g`). It belongs to this binding, so leaving the
    // page or binding another task collapses it.
    this.trailOpen = false;
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
  // `extra` stops follow `+ step` (the task page's PAPER TRAIL heading, `"trail"`).
  move(task, delta, extra = []) {
    const ids = [...this.rows(task).map((step) => step.id), "add", ...extra];
    let i = ids.indexOf(this.selected);
    if (i < 0) i = delta > 0 ? -1 : 0;
    this.selected = ids[(i + delta + ids.length) % ids.length];
    this.marked = null;
  }
  begin(task, id = null) {
    this.bind(task);
    if (id) this.staged ??= structuredClone(task.steps);
    this.selected = id ?? "add";
    this.editor = {
      id,
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
        task.steps.push(step);
        if (this.staged) this.staged.push(structuredClone(step));
        this.selected = step.id;
        this.editor = finish ? null : { id: null, text: "" };
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
