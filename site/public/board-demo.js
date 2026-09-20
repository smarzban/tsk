import { TaskSteps } from "./task-steps.js";
import { wrapText } from "./board-wrap.js";
import { parseCapture } from "./capture.js";

(() => {
  const root = document.getElementById("tsk-demo");
  const frame = document.getElementById("board-demo");
  if (!root || !frame) return;

  const WIDE_SPLIT_MIN_COLUMNS = 110;

  const GLYPH = {
    open: "◌",
    ready: "○",
    started: "●",
    blocked: "■",
    review: "▲",
    done: "✓",
  };

  const TABS = [
    ["desk", "desk"],
    ["project", "selected project"],
    ["projects", "projects"],
  ];
  // Optional fixture data is supplied only by the local parity harness.
  const fixture = JSON.parse(
    document.getElementById("tsk-demo-fixture")?.textContent || "null",
  );
  const clock = () => fixture?.now ?? Date.now();
  const launchProject = () => fixture?.selectedProject || "launchpad";
  const isProjectsThreadView = () =>
    state.tab === "projects" && state.projectView !== null;
  const NOW = clock();
  const MIN = 60 * 1000;
  const HOUR = 60 * MIN;
  const DAY = 24 * HOUR;

  const seed = () => {
    let n = 12;
    const task = (partial) => ({
      id: `t${n}`,
      number: n++,
      notes: "",
      steps: [],
      thread: null,
      project: null,
      archived: false,
      createdAt: NOW - 3 * MIN,
      updatedAt: NOW - 3 * MIN,
      ...partial,
    });
    return [
      task({
        title: "Rotate refresh tokens on every use",
        status: "started",
        project: "launchpad",
        thread: "auth",
        notes:
          "Store a hash of each refresh token and revoke its token family if an old token is reused.",
        steps: [
          {
            id: "sample-0-0",
            text: "Add token-family storage",
            done: true,
          },
          {
            id: "sample-0-1",
            text: "Rotate tokens in the refresh endpoint",
            done: false,
          },
          {
            id: "sample-0-2",
            text: "Test replay and expiry",
            done: false,
          },
        ],
      }),
      task({
        title: "Make webhook delivery idempotent",
        status: "ready",
        project: "launchpad",
        thread: "api",
        notes:
          "Retries can deliver the same event twice. Deduplicate by event ID before applying side effects.",
        steps: [
          {
            id: "sample-1-0",
            text: "Reproduce duplicate delivery",
            done: false,
          },
          {
            id: "sample-1-1",
            text: "Persist processed event IDs",
            done: false,
          },
          {
            id: "sample-1-2",
            text: "Test concurrent retries",
            done: false,
          },
        ],
      }),
      task({
        title: "Investigate slow integration tests",
        status: "open",
        project: null,
        thread: null,
        notes:
          "The API suite takes twelve minutes in CI. Measure where the time goes before changing the runner.",
        steps: [
          {
            id: "sample-2-0",
            text: "Capture per-test timings",
            done: false,
          },
          {
            id: "sample-2-1",
            text: "Find repeated database setup",
            done: false,
          },
          {
            id: "sample-2-2",
            text: "Compare the next CI run",
            done: false,
          },
        ],
      }),
      task({
        title: "Unblock OIDC login in staging",
        status: "blocked",
        project: "launchpad",
        thread: "auth",
        notes:
          "Waiting for the identity provider's staging client registration. The callback handler is ready.",
        steps: [
          {
            id: "sample-3-0",
            text: "Verify the redirect URI",
            done: true,
          },
          {
            id: "sample-3-1",
            text: "Register the staging client",
            done: false,
          },
          {
            id: "sample-3-2",
            text: "Run the end-to-end login test",
            done: false,
          },
        ],
      }),
      task({
        title: "Review API key scope checks",
        status: "review",
        project: "launchpad",
        thread: "auth",
        notes:
          "The middleware now checks scopes before routing. Review the denial paths and ensure logs never include credentials.",
        steps: [
          {
            id: "sample-4-0",
            text: "Add scope middleware",
            done: true,
          },
          {
            id: "sample-4-1",
            text: "Test missing and insufficient scopes",
            done: false,
          },
          {
            id: "sample-4-2",
            text: "Review the audit log fields",
            done: false,
          },
        ],
      }),
      task({
        title: "Generate the TypeScript API client",
        status: "blocked",
        project: "website",
        thread: "api",
        notes:
          "Waiting for the pagination schema to be finalized before generating the client used by the console.",
        steps: [
          {
            id: "sample-5-0",
            text: "Validate the OpenAPI schema",
            done: true,
          },
          {
            id: "sample-5-1",
            text: "Generate typed request methods",
            done: false,
          },
          {
            id: "sample-5-2",
            text: "Run the client against staging",
            done: false,
          },
        ],
      }),
      task({
        title: "Review error states in the API console",
        status: "review",
        project: "website",
        thread: "api",
        notes:
          "Check how the console handles expired sessions, rate limits, and server errors without losing request input.",
        steps: [
          {
            id: "sample-6-0",
            text: "Handle 401 and 429 responses",
            done: true,
          },
          {
            id: "sample-6-1",
            text: "Preserve the request body on failure",
            done: false,
          },
          {
            id: "sample-6-2",
            text: "Check retry and loading states",
            done: false,
          },
        ],
      }),
      task({
        title: "Add cursor pagination to the events endpoint",
        status: "open",
        project: "launchpad",
        thread: "api",
        notes:
          "Use a stable cursor for events with matching timestamps. Keep the response contract backwards compatible.",
        steps: [
          {
            id: "sample-7-0",
            text: "Add the cursor query",
            done: false,
          },
          {
            id: "sample-7-1",
            text: "Cover equal-timestamp boundaries",
            done: false,
          },
          {
            id: "sample-7-2",
            text: "Document next-page tokens",
            done: false,
          },
        ],
      }),
      task({
        title: "Add a health check for the worker pool",
        status: "done",
        project: "launchpad",
        thread: "reliability",
        notes:
          "Readiness now fails when workers stop consuming jobs; liveness stays independent.",
        steps: [
          {
            id: "sample-8-0",
            text: "Expose readiness and liveness",
            done: true,
          },
          {
            id: "sample-8-1",
            text: "Test a stalled worker",
            done: true,
          },
        ],
      }),
      task({
        title: "Fix the console build cache",
        status: "done",
        project: "website",
        thread: "tooling",
        notes:
          "Cache dependencies by lockfile and runtime version so CI does not reuse incompatible artifacts.",
        steps: [
          {
            id: "sample-9-0",
            text: "Update the cache key",
            done: true,
          },
          {
            id: "sample-9-1",
            text: "Verify a clean CI build",
            done: true,
          },
        ],
      }),
      task({
        title: "Prototype a GraphQL gateway",
        status: "ready",
        project: "launchpad",
        thread: "api",
        notes:
          "Keep the prototype for reference while the REST API stabilizes.",
        steps: [
          {
            id: "sample-10-0",
            text: "Map the existing endpoints",
            done: false,
          },
          {
            id: "sample-10-1",
            text: "Measure query overhead",
            done: false,
          },
        ],
        archived: true,
      }),
      task({
        title: "Plan the support handoff",
        status: "ready",
        thread: "triage",
        notes:
          "Pick the owner, escalation path, and first response expectations before the pilot opens.",
      }),
      task({
        title: "Clean up stale local branches",
        status: "ready",
        notes:
          "Keep only the branches needed for the release train and document anything retained.",
      }),
      task({
        title: "Sort feedback from the pilot",
        status: "open",
        thread: "triage",
        notes:
          "Group feedback by workflow before deciding which issues to pick next.",
      }),
      task({
        title: "Record the retry runbook",
        status: "open",
        notes:
          "Capture the safe retry steps while the incident details are still fresh.",
      }),
      task({
        title: "Add audit events for admin changes",
        status: "ready",
        project: "launchpad",
        thread: "ops",
        notes: "Record actor, target, and result for each privileged change.",
      }),
      task({
        title: "Trace worker queue saturation",
        status: "open",
        project: "launchpad",
        thread: "ops",
        notes:
          "Measure queue depth and worker lag before choosing a scaling threshold.",
      }),
      task({
        title: "Document regional failover checks",
        status: "open",
        project: "launchpad",
        thread: "ops",
        notes:
          "Write the operator checks that confirm a region can take traffic safely.",
      }),
    ];
  };

  const state = {
    tasks: fixture ? structuredClone(fixture.tasks) : seed(),
    tab: "desk",
    // The focused scope is transient, while this is the project selected by tab 2.
    selectedProject: launchProject(),
    focusProject: null,
    searchQuery: "",
    searchPinned: false,
    threadFilter: null,
    projectView: null,
    filterI: 0,
    collapsed: new Set(),
    selectedId: "t1",
    markMode: false,
    markedIds: new Set(),
    pendingDelete: null,
    pendingDeleteBulk: false,
    message: "",
    peekId: null,
    flashId: null,
    copyNotice: "",
    drawer: false,
    archivedOpen: false,
    inboxOpen: true,
    overlay: null,
    draft: "",
    paletteQ: "",
    paletteI: 0,
    helpQ: "",
    pickerI: 0,
    editField: null,
    editDraft: "",
    quickExpanded: false,
    capture: null,
    quickStage: null,
    undo: null,
    refuse: "",
    quickOwner: "outer",
    nextId: 20,
    nextNumber: 22,
    // Wide stage slider: board · split · rail · page. Focus is the stage.
    stage: "board",
    stageOrigin: null,
  };

  const steps = new TaskSteps();
  const previewSteps = new TaskSteps();
  const preview = {
    project: null,
    selectedId: null,
    markMode: false,
    markedIds: new Set(),
    pendingDelete: null,
    pendingDeleteBulk: false,
    threadFilter: null,
    peekId: null,
    drawer: false,
    archivedOpen: false,
    page: false,
    editField: null,
    editDraft: "",
    undo: null,
    lastClick: 0,
    message: "",
    searchQuery: "",
    searchPinned: false,
  };

  function projectsOverview() {
    return (
      state.tab === "projects" &&
      state.projectView === null &&
      !state.focusProject
    );
  }
  function projectsPreviewActive() {
    return (
      projectsOverview() &&
      isWideSplit() &&
      ["split", "rail"].includes(state.stage)
    );
  }
  function projectsPreviewFocused() {
    return projectsPreviewActive() && state.stage === "rail";
  }
  function projectsPreviewPage() {
    return projectsPreviewFocused() && preview.page;
  }
  const TASK_STAGES = ["rail", "page"];
  function taskFocus() {
    return !projectsOverview() && TASK_STAGES.includes(state.stage);
  }
  function stageRight() {
    if (projectsOverview()) {
      if (state.stage === "board") openProjectPreview();
      else if (state.stage === "split") state.stage = "rail";
      return;
    }
    if (!state.selectedId) return;
    if (state.stage === "board") state.stage = "split";
    else if (state.stage === "split") state.stage = "rail";
    else if (state.stage === "rail") {
      state.stageOrigin = "rail";
      state.stage = "page";
    }
  }
  function stageLeft() {
    if (projectsOverview()) {
      if (state.stage === "rail") state.stage = "split";
      else if (state.stage === "split") {
        if (!previewHasUnsavedWork()) dropProjectPreview();
        state.stage = "board";
      }
      return;
    }
    if (state.stage === "split") state.stage = "board";
    else if (state.stage === "rail") state.stage = "split";
    else if (state.stage === "page") {
      state.stageOrigin = null;
      state.stage = "rail";
    }
  }
  function openFullPage() {
    if (!state.selectedId) return;
    if (state.stage !== "page") state.stageOrigin = state.stage;
    state.stage = "page";
  }
  function leaveTaskPage() {
    steps.reset();
    if (state.stage === "page") {
      state.stage = state.stageOrigin || "board";
      state.stageOrigin = null;
    } else if (state.stage === "rail") state.stage = "split";
  }
  function enterTaskStage() {
    if (state.stage === "board") {
      state.stageOrigin = "board";
      state.stage = "page";
    } else if (state.stage === "split") state.stage = "rail";
  }

  const esc = (s) =>
    String(s).replace(
      /[&<>"']/g,
      (c) =>
        ({
          "&": "&amp;",
          "<": "&lt;",
          ">": "&gt;",
          '"': "&quot;",
          "'": "&#39;",
        })[c],
    );

  const age = (ts) => {
    const secs = Math.max(0, Math.floor((clock() - ts) / 1000));
    if (secs < 60) return `${secs}s`;
    if (secs < 3600) return `${Math.floor(secs / 60)}m`;
    if (secs < 86400) return `${Math.floor(secs / 3600)}h`;
    return `${Math.floor(secs / 86400)}d`;
  };

  const projectName = (task) => task.project || "desk";
  // Same rule as the TUI: status sections show the newest status change first (statusAt
  // moves only in setStatus, never on an edit or a step), ready and inbox backlogs are FIFO
  // by pick or capture time. Ties break by createdAt then id so the order is stable.
  const statusAt = (t) => t.statusAt ?? t.createdAt;
  const byStatusChange = (a, b) =>
    statusAt(b) - statusAt(a) ||
    a.createdAt - b.createdAt ||
    String(a.id).localeCompare(String(b.id));
  const byCreated = (a, b) =>
    a.createdAt - b.createdAt || String(a.id).localeCompare(String(b.id));
  const taskById = (id) => state.tasks.find((t) => t.id === id);

  function pickerOptions() {
    const set = new Set();
    for (const t of state.tasks) if (t.project) set.add(t.project);
    return ["desk", ...[...set].sort()];
  }

  function projectNames() {
    const names = new Set(
      state.tasks.filter((t) => t.project && !t.archived).map((t) => t.project),
    );
    return [...names].sort((a, b) => {
      if (a === launchProject()) return -1;
      if (b === launchProject()) return 1;
      return a.localeCompare(b);
    });
  }

  function projectPath(name) {
    return (
      state.tasks.find((t) => t.project === name)?.scope?.project?.path ||
      `/projects/${name}`
    );
  }
  function matchingProjectNames() {
    const query = state.searchQuery.trim().toLowerCase();
    return projectNames().filter(
      (name) =>
        !query ||
        name.toLowerCase().includes(query) ||
        projectPath(name).toLowerCase().includes(query),
    );
  }

  function taskMatchesSearch(task, query) {
    const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
    if (!words.length) return true;
    const content = [
      task.title,
      task.notes,
      ...(task.steps || []).map((step) => step.text),
      task.thread,
      `T${task.number}`,
    ]
      .filter(Boolean)
      .join("\n")
      .toLowerCase();
    return words.every((word) => content.includes(word));
  }

  function searchOwner() {
    return projectsPreviewFocused() ? preview : state;
  }

  function updateSearchQuery(owner, value) {
    const previousRows = owner === preview ? previewRows() : buildRows();
    owner.searchQuery = value;
    if (owner === preview) ensurePreviewSelection(previewRows(), previousRows);
    else ensureSelection(buildRows(), previousRows);
  }

  function pinSearch(owner) {
    if (!owner.searchQuery.trim()) updateSearchQuery(owner, "");
    owner.searchPinned = Boolean(owner.searchQuery);
  }

  function selectedTask() {
    if (state.quickExpanded && state.quickOwner === "outer")
      return state.capture;
    return taskById(state.selectedId) || null;
  }

  function targetTasks(form = state) {
    const marked = form.markMode
      ? [...form.markedIds].map(taskById).filter(Boolean)
      : [];
    if (marked.length) return marked;
    const selected = form === preview ? previewTask() : selectedTask();
    return selected ? [selected] : [];
  }

  function clearMarks(form = state) {
    form.markMode = false;
    form.markedIds.clear();
  }

  function toggleMarkMode(form = state) {
    if (form.markMode) clearMarks(form);
    else form.markMode = true;
  }

  function markCurrent(form = state) {
    const id = form.selectedId;
    if (id && taskById(id)) form.markedIds.add(id);
  }

  function toggleMark(form = state) {
    const id = form.selectedId;
    if (!id || !taskById(id)) return;
    if (form.markedIds.has(id)) form.markedIds.delete(id);
    else form.markedIds.add(id);
  }

  function previewTask() {
    if (state.quickExpanded && state.quickOwner === "preview")
      return state.capture;
    return taskById(preview.selectedId) || null;
  }

  function previewProjectTasks() {
    if (!preview.project) return [];
    return state.tasks.filter(
      (task) => task.project === preview.project && !task.archived,
    );
  }

  function previewMatchesThread(task) {
    return (
      preview.threadFilter === null ||
      (task.thread || "") === preview.threadFilter
    );
  }

  function previewVisibleTasks() {
    const scoped = previewProjectTasks().filter(previewMatchesThread);
    const live = scoped.filter((task) => taskMatchesSearch(task, preview.searchQuery));
    const done = preview.drawer
      ? scoped.filter((task) => taskMatchesSearch(task, preview.searchQuery))
      : scoped;
    return {
      started: live
        .filter((task) => task.status === "started")
        .sort(byStatusChange),
      review: live
        .filter((task) => task.status === "review")
        .sort(byStatusChange),
      blocked: live
        .filter((task) => task.status === "blocked")
        .sort(byStatusChange),
      ready: live.filter((task) => task.status === "ready").sort(byCreated),
      done: done
        .filter((task) => task.status === "done")
        .sort(byStatusChange),
    };
  }

  function previewRows() {
    const rows = [];
    const v = previewVisibleTasks();
    const pushHeader = (label, count) =>
      rows.push({ kind: "section", label, count, selectable: false });
    const pushTask = (task, dim = false) =>
      rows.push({ kind: "task", task, selectable: true, id: task.id, dim });
    const need = [...v.review, ...v.blocked].sort(byStatusChange);
    if (need.length) {
      pushHeader("NEEDS YOU", need.length);
      need.forEach((task) => pushTask(task));
    }
    if (v.started.length) {
      pushHeader("IN MOTION", v.started.length);
      v.started.forEach((task) => pushTask(task));
    }
    if (v.ready.length || !preview.searchQuery.trim())
      pushHeader("ON DECK", v.ready.length);
    v.ready.forEach((task) => pushTask(task));
    if (preview.drawer) {
      if (v.done.length) pushHeader("DONE", v.done.length);
      v.done.forEach((task) => pushTask(task));
      const archived = state.tasks
        .filter(
          (task) =>
            task.project === preview.project &&
            task.archived &&
            previewMatchesThread(task) &&
            taskMatchesSearch(task, preview.searchQuery),
        )
        .sort(byStatusChange);
      if (archived.length) {
        rows.push({
          kind: "archived",
          label: "archived",
          count: archived.length,
          selectable: true,
          id: "nav:preview-archived",
        });
        if (preview.archivedOpen)
          archived.forEach((task) => pushTask(task, true));
      }
    }
    return rows;
  }

  function previewFilterOptions() {
    const tasks = state.tasks.filter(
      (task) =>
        task.project === preview.project &&
        !task.archived &&
        task.status !== "done",
    );
    const names = [
      ...new Set(tasks.map((task) => task.thread).filter(Boolean)),
    ].sort(
      (a, b) =>
        tasks.filter((task) => task.thread === b).length -
          tasks.filter((task) => task.thread === a).length ||
        a.localeCompare(b),
    );
    return [
      { value: null, label: `All tasks  ${tasks.length}` },
      ...names.map((name) => ({
        value: name,
        label: `#${name}  ${tasks.filter((task) => task.thread === name).length}`,
      })),
      {
        value: "",
        label: `Without a thread  ${tasks.filter((task) => !task.thread).length}`,
      },
    ];
  }

  function previewFilterLabel() {
    return preview.threadFilter === null
      ? "all"
      : preview.threadFilter === ""
        ? "without a thread"
        : `#${preview.threadFilter}`;
  }

  function openPreviewFilter() {
    if (!projectsPreviewFocused() || previewHasUnsavedWork()) return;
    state.overlay = "preview-filter";
    state.filterI = Math.max(
      0,
      previewFilterOptions().findIndex(
        (option) => option.value === preview.threadFilter,
      ),
    );
  }

  function choosePreviewFilter(index) {
    const option = previewFilterOptions()[index];
    if (!option) return;
    clearMarks(preview);
    preview.threadFilter = option.value;
    preview.peekId = null;
    ensurePreviewSelection();
    state.overlay = null;
  }

  function renderPreviewFilter() {
    return `<div class="tsk-box" role="dialog" aria-label="project thread filter"><div class="tsk-box-top"><span class="tsk-box-title">project thread</span><button type="button" class="tsk-box-close" data-close="1">[x]</button></div><div class="tsk-box-body">${previewFilterOptions()
      .map(
        (option, index) =>
          `<div class="tsk-pal-row" data-preview-filter-option="${index}"><span class="${index === state.filterI ? "sel-text" : ""}">${index === state.filterI ? "▸" : " "} ${esc(option.label)}</span></div>`,
      )
      .join(
        "",
      )}</div><div class="tsk-box-foot">↑↓ move · enter choose · esc close</div></div>`;
  }

  function previewSelectableIds(rows = previewRows()) {
    return rows.filter((row) => row.selectable).map((row) => row.id);
  }

  function ensurePreviewSelection(rows = previewRows(), previousRows = null) {
    const ids = previewSelectableIds(rows);
    if (!ids.length) {
      preview.selectedId = null;
      return;
    }
    if (!ids.includes(preview.selectedId)) {
      preview.selectedId = nearestVisibleId(
        preview.selectedId,
        previousRows ? previewSelectableIds(previousRows) : [],
        ids,
      );
    }
  }

  function previewHasUnsavedWork() {
    return Boolean(
      preview.editField ||
      previewSteps.dirty ||
      previewSteps.editor ||
      (state.overlay === "quick" && state.quickOwner === "preview"),
    );
  }

  let renderColumns = null;
  function terminalColumns() {
    if (renderColumns !== null) return renderColumns;
    const style = getComputedStyle(root);
    const canvas = document.createElement("canvas");
    const context = canvas.getContext("2d");
    context.font = `${style.fontSize} ${style.fontFamily}`;
    const cell = context.measureText("0").width;
    const padding =
      parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
    return Math.round((root.clientWidth - padding) / cell);
  }

  function isWideSplit() {
    return terminalColumns() >= WIDE_SPLIT_MIN_COLUMNS;
  }

  function fallbackProject() {
    if (state.quickOwner === "preview") return preview.project;
    if (state.focusProject) return state.focusProject;
    return null;
  }

  function archivedInScope() {
    const archived = state.tasks
      .filter(
        (t) =>
          t.archived &&
          matchesThread(t) &&
          (!state.drawer || taskMatchesSearch(t, state.searchQuery)),
      )
      .sort(byStatusChange);
    if (state.focusProject || isProjectsThreadView()) {
      const inP = (t) =>
        !state.focusProject ||
        (state.focusProject === "desk"
          ? !t.project
          : t.project === state.focusProject);
      return archived.filter(inP);
    }
    return archived;
  }

  function filterOptions() {
    const project = state.focusProject;
    const tasks = state.tasks.filter(
      (t) =>
        !t.archived &&
        t.status !== "done" &&
        (!project || t.project === project),
    );
    const names = [...new Set(tasks.map((t) => t.thread).filter(Boolean))].sort(
      (a, b) =>
        tasks.filter((t) => t.thread === b).length -
          tasks.filter((t) => t.thread === a).length || a.localeCompare(b),
    );
    return [
      {
        value: null,
        label: project ? `All tasks  ${tasks.length}` : "Overview",
      },
      ...names.map((name) => ({
        value: name,
        label: `#${name}  ${tasks.filter((t) => t.thread === name).length}`,
      })),
      ...(project
        ? [
            {
              value: "",
              label: `Without a thread  ${tasks.filter((t) => !t.thread).length}`,
            },
          ]
        : []),
    ];
  }
  function openFilter() {
    if (steps.dirty || steps.editor || previewHasUnsavedWork()) return;
    state.overlay = "filter";
    const value = state.focusProject ? state.threadFilter : state.projectView;
    state.filterI = Math.max(
      0,
      filterOptions().findIndex((o) => o.value === value),
    );
  }
  function chooseFilter(index) {
    const option = filterOptions()[index];
    if (!option) return;
    clearMarks();
    if (state.focusProject) state.threadFilter = option.value;
    else {
      state.projectView = option.value;
      state.searchQuery = "";
      state.searchPinned = false;
    }
    state.overlay = null;
    state.peekId = null;
    state.stage = "board";
    if (state.tab === "projects") dropProjectPreview();
  }
  function renderFilter() {
    const title = state.focusProject ? "thread filter" : "projects View";
    return `<div class="tsk-box" role="dialog" aria-label="${title}"><div class="tsk-box-top"><span class="tsk-box-title">${title}</span><button type="button" class="tsk-box-close" data-close="1">[x]</button></div><div class="tsk-box-body">${filterOptions()
      .map(
        (o, i) =>
          `<div class="tsk-pal-row" data-filter-option="${i}"><span class="${i === state.filterI ? "sel-text" : ""}">${i === state.filterI ? "▸" : " "} ${esc(o.label)}</span></div>`,
      )
      .join(
        "",
      )}</div><div class="tsk-box-foot">↑↓ move · enter choose · esc close</div></div>`;
  }
  const matchesThread = (t) =>
    state.focusProject
      ? state.threadFilter === null || (t.thread || "") === state.threadFilter
      : state.tab !== "projects" ||
        state.projectView === null ||
        t.thread === state.projectView;
  function visibleTasks() {
    // Hidden (archived) tasks leave every working view. The closed drawer keeps
    // its unfiltered count; opening it brings done tasks into content search.
    const open = state.tasks.filter(
      (t) =>
        t.status !== "done" &&
        !t.archived &&
        matchesThread(t) &&
        taskMatchesSearch(t, state.searchQuery),
    );
    const done = state.tasks
      .filter(
        (t) =>
          t.status === "done" &&
          !t.archived &&
          matchesThread(t) &&
          (!state.drawer || taskMatchesSearch(t, state.searchQuery)),
      )
      .sort(byStatusChange);
    if (state.focusProject || isProjectsThreadView()) {
      const inP = (t) =>
        !state.focusProject ||
        (state.focusProject === "desk"
          ? !t.project
          : t.project === state.focusProject);
      return {
        started: open
          .filter((t) => inP(t) && t.status === "started")
          .sort(byStatusChange),
        review: open
          .filter((t) => inP(t) && t.status === "review")
          .sort(byStatusChange),
        blocked: open
          .filter((t) => inP(t) && t.status === "blocked")
          .sort(byStatusChange),
        ready: open
          .filter((t) => inP(t) && t.status === "ready")
          .sort(byCreated),
        inbox: open
          .filter((t) => inP(t) && t.status === "open")
          .sort(byCreated),
        done: done.filter(inP),
      };
    }
    if (state.tab === "desk") {
      return {
        started: open
          .filter((t) => t.status === "started")
          .sort(byStatusChange),
        need: open
          .filter((t) => t.status === "blocked" || t.status === "review")
          .sort(byStatusChange),
        desk: open
          .filter((t) => !t.project && t.status === "ready")
          .sort(byCreated),
        inbox: open
          .filter((t) => !t.project && t.status === "open")
          .sort(byCreated),
        done,
      };
    }
    return { open, done };
  }

  function buildRows() {
    const rows = [];
    const pushHeader = (kind, label, count, extra = {}) => {
      rows.push({ kind, label, count, selectable: false, ...extra });
    };
    const pushTask = (task, indent = 0) => {
      rows.push({ kind: "task", task, indent, selectable: true, id: task.id });
    };

    if (state.tab === "desk" && !state.focusProject) {
      const v = visibleTasks();
      if (v.need.length) {
        pushHeader("section", "NEEDS YOU", v.need.length);
        v.need.forEach((t) => pushTask(t));
      }
      if (v.started.length)
        pushHeader("section", "IN MOTION", v.started.length);
      v.started.forEach((t) => pushTask(t));
      if (
        v.desk.length ||
        v.inbox.length ||
        (!v.need.length && !state.searchQuery.trim())
      )
        pushHeader("section", "ON DECK · desk", v.desk.length + v.inbox.length);
      v.desk.forEach((t) => pushTask(t));
      if (v.inbox.length) {
        rows.push({
          kind: "inbox",
          label: "inbox",
          count: v.inbox.length,
          selectable: true,
          id: "nav:inbox",
        });
        if (state.inboxOpen) v.inbox.forEach((t) => pushTask(t));
      }
      if (state.drawer) {
        if (v.done.length) pushHeader("section", "DONE", v.done.length);
        v.done.forEach((t) => pushTask(t));
        const archived = archivedInScope();
        if (archived.length) {
          rows.push({
            kind: "archived",
            label: "archived",
            count: archived.length,
            selectable: true,
            id: "nav:archived",
          });
          if (state.archivedOpen)
            archived.forEach((t) =>
              rows.push({
                kind: "task",
                task: t,
                indent: 0,
                selectable: true,
                id: t.id,
                dim: true,
              }),
            );
        }
      }
      return rows;
    }

    if (state.focusProject || isProjectsThreadView()) {
      const v = visibleTasks();
      const need = [...v.review, ...v.blocked].sort(byStatusChange);
      if (need.length) {
        pushHeader("section", "NEEDS YOU", need.length);
        need.forEach((t) => pushTask(t));
      }
      if (v.started.length)
        pushHeader("section", "IN MOTION", v.started.length);
      v.started.forEach((t) => pushTask(t));
      if (v.ready.length || v.inbox.length || !state.searchQuery.trim())
        pushHeader("section", "ON DECK", v.ready.length + v.inbox.length);
      v.ready.forEach((t) => pushTask(t));
      if (v.inbox.length) {
        rows.push({
          kind: "inbox",
          label: "inbox",
          count: v.inbox.length,
          selectable: true,
          id: "nav:inbox",
        });
        if (state.inboxOpen) v.inbox.forEach((t) => pushTask(t));
      }
      if (state.drawer) {
        if (v.done.length) pushHeader("section", "DONE", v.done.length);
        v.done.forEach((t) => pushTask(t));
        const archived = archivedInScope();
        if (archived.length) {
          rows.push({
            kind: "archived",
            label: "archived",
            count: archived.length,
            selectable: true,
            id: "nav:archived",
          });
          if (state.archivedOpen)
            archived.forEach((t) =>
              rows.push({
                kind: "task",
                task: t,
                indent: 0,
                selectable: true,
                id: t.id,
                dim: true,
              }),
            );
        }
      }
      return rows;
    }

    if (state.tab === "projects") {
      for (const name of matchingProjectNames()) {
        const group = state.tasks.filter(
          (t) => (t.project || "desk") === name && !t.archived,
        );
        const open = group.filter((t) => t.status !== "done");
        const started = open
          .filter((t) => t.status === "started")
          .sort(byStatusChange);
        const review = open
          .filter((t) => t.status === "review")
          .sort(byStatusChange);
        const blocked = open
          .filter((t) => t.status === "blocked")
          .sort(byStatusChange);
        const onDeck = open.filter(
          (t) => t.status === "ready" || t.status === "open",
        );
        const done = group.filter((t) => t.status === "done");
        rows.push({
          kind: "project",
          label: name,
          project: name,
          needs: review.length + blocked.length,
          motion: started.length,
          onDeck: onDeck.length,
          done: done.length,
          selectable: true,
          id: `project:${name}`,
        });
        // Project index rows are navigation, not collapsible task groups.
      }
      if (state.drawer) {
        const done = state.tasks
          .filter((t) => t.status === "done" && !t.archived && matchesThread(t))
          .sort(byStatusChange);
        pushHeader("section", "DONE", done.length);
        done.forEach((t) => pushTask(t));
        const archived = archivedInScope();
        if (archived.length) {
          rows.push({
            kind: "archived",
            label: "archived",
            count: archived.length,
            selectable: true,
            id: "nav:archived",
          });
          if (state.archivedOpen)
            archived.forEach((t) =>
              rows.push({
                kind: "task",
                task: t,
                indent: 0,
                selectable: true,
                id: t.id,
                dim: true,
              }),
            );
        }
      }
      return rows;
    }

    if (state.drawer) {
      const done = state.tasks
        .filter((t) => t.status === "done" && !t.archived && matchesThread(t))
        .sort(byStatusChange);
      pushHeader("section", "DONE", done.length);
      done.forEach((t) => pushTask(t));
      const archived = archivedInScope();
      if (archived.length) {
        rows.push({
          kind: "archived",
          label: "archived",
          count: archived.length,
          selectable: true,
          id: "nav:archived",
        });
        if (state.archivedOpen)
          archived.forEach((t) =>
            rows.push({
              kind: "task",
              task: t,
              indent: 0,
              selectable: true,
              id: t.id,
              dim: true,
            }),
          );
      }
    }
    return rows;
  }

  function selectableIds(rows) {
    return rows.filter((r) => r.selectable).map((r) => r.id);
  }

  function nearestVisibleId(previous, previousIds, nextIds) {
    if (!nextIds.length) return null;
    if (nextIds.includes(previous)) return previous;
    const index = previousIds.indexOf(previous);
    if (index < 0) return nextIds[0];
    const horizon = Math.max(index, previousIds.length - index - 1);
    for (let distance = 1; distance <= horizon; distance += 1) {
      const before = previousIds[index - distance];
      if (before && nextIds.includes(before)) return before;
      const after = previousIds[index + distance];
      if (after && nextIds.includes(after)) return after;
    }
    return nextIds[Math.min(index, nextIds.length - 1)];
  }

  function ensureSelection(rows, previousRows = null) {
    const ids = selectableIds(rows);
    if (!ids.length) {
      state.selectedId = null;
      return;
    }
    if (!ids.includes(state.selectedId)) {
      state.selectedId = nearestVisibleId(
        state.selectedId,
        previousRows ? selectableIds(previousRows) : [],
        ids,
      );
    }
  }

  function metaFor(task) {
    const bits = [];
    if (!state.focusProject && task.project) bits.push(projectName(task));
    if (state.focusProject && task.thread) bits.push(`#${task.thread}`);
    return bits.join(" · ");
  }

  function showCopyNotice(message) {
    state.copyNotice = message;
    setTimeout(() => {
      if (state.copyNotice === message) {
        state.copyNotice = "";
        render();
      }
    }, 2000);
    render();
  }

  function copyTaskIdentifier(task) {
    const identifier = `T${task.number}`;
    if (!navigator.clipboard?.writeText) {
      showCopyNotice("copy unavailable");
      return;
    }
    navigator.clipboard
      .writeText(identifier)
      .then(() => showCopyNotice(`copied ${identifier}`))
      .catch(() => showCopyNotice("copy failed"));
  }

  function verbItems(task) {
    if (!task) {
      // The bar is a prompt, not a keymap: open · status verbs · add · help.
      if (state.tab === "projects" && !state.focusProject) {
        return [
          { id: "open", label: "enter open" },
          { id: "search", label: "/ search" },
          { id: "help", label: "? help" },
        ];
      }
      return [
        { id: "capture", label: "+ add" },
        { id: "help", label: "? help" },
      ];
    }
    const items = [{ id: "open", label: "enter open" }];
    if (task.status === "open" || task.status === "ready")
      items.push({ id: "start", label: "s start" });
    if (task.status !== "ready") items.push({ id: "ready", label: "n ready" });
    if (task.status !== "open") items.push({ id: "inbox", label: "o inbox" });
    if (task.status !== "done") {
      items.push({ id: "done", label: "d done" });
      items.push({
        id: "block",
        label: task.status === "blocked" ? "b unblock" : "b block",
      });
    }
    items.push(
      { id: "capture", label: "+ add" },
      { id: "help", label: "? help" },
    );
    return items;
  }

  function runVerb(id) {
    if (id === "search") {
      searchOwner().searchPinned = false;
      state.overlay = "search";
    }
    if (id === "open" && state.selectedId) {
      const row = selectedRow();
      if (row?.kind === "project") openProject(row.project);
      else openFullPage();
    }
    if (id === "capture") openQuickAdd();
    if (id === "help") {
      state.overlay = "help";
      state.helpQ = "";
    }
    if (id === "palette") {
      state.overlay = "palette";
      state.paletteQ = "";
      state.paletteI = 0;
    }
    if (id === "start") primaryVerb();
    if (id === "ready") setStatus("ready");
    if (id === "inbox") setStatus("open");
    if (id === "done") setStatus("done");
    if (id === "block") toggleBlock();
    if (id === "file") fileSelected();
  }

  function paletteCommands() {
    const q = state.paletteQ.trim().toLowerCase();
    const all = [
      {
        id: "open",
        label: "set status: open",
        run: () => setStatus("open"),
      },
      {
        id: "ready",
        label: "set status: ready",
        run: () => setStatus("ready"),
      },
      {
        id: "started",
        label: "set status: started",
        run: () => setStatus("started"),
      },
      {
        id: "blocked",
        label: "set status: blocked",
        run: () => setStatus("blocked"),
      },
      {
        id: "review",
        label: "set status: review",
        run: () => setStatus("review"),
      },
      { id: "done", label: "set status: done", run: () => setStatus("done") },
      { id: "desk", label: "go desk", run: () => goTab("desk") },
      { id: "projects", label: "go projects", run: () => goTab("projects") },
      {
        id: "project",
        label: "go selected project",
        run: () => goTab("project"),
      },
      { id: "capture", label: "capture", run: () => openQuickAdd() },
      {
        id: "help",
        label: "help",
        run: () => {
          state.overlay = "help";
          state.helpQ = "";
        },
      },
      { id: "reset", label: "reset demo", run: resetDemo },
    ];
    if (!state.focusProject && state.tab === "projects") {
      all.push({ id: "groups", label: "toggle groups", run: toggleAllGroups });
    }
    return all.filter((c) => !q || c.label.includes(q) || c.id.includes(q));
  }

  function rememberUndo(form, tasks) {
    form.undo = {
      items: tasks.map((task) => ({
        task: { ...task },
        index: state.tasks.indexOf(task),
      })),
    };
  }

  function flashTasks(tasks) {
    const last = tasks.at(-1);
    if (!last) return;
    state.flashId = last.id;
    setTimeout(() => {
      if (state.flashId === last.id) {
        state.flashId = null;
        render();
      }
    }, 420);
  }

  function setStatus(status) {
    const tasks = targetTasks();
    const changed = tasks.filter((task) => task.status !== status);
    if (status === "done" && changed.length) rememberUndo(state, changed);
    for (const task of changed) {
      task.status = status;
      task.updatedAt = clock();
      task.statusAt = task.updatedAt;
    }
    clearMarks();
    state.pendingDelete = null;
    state.message = "";
    flashTasks(changed);
  }

  function goTab(tab) {
    if (steps.dirty || steps.editor || previewHasUnsavedWork()) return;
    clearMarks();
    clearMarks(preview);
    state.threadFilter = null;
    state.tab = tab;
    state.focusProject = tab === "project" ? state.selectedProject : null;
    state.searchQuery = "";
    state.searchPinned = false;
    state.peekId = null;
    state.overlay = null;
    state.quickOwner = "outer";
    state.stage = "board";
    state.stageOrigin = null;
    dropProjectPreview();
  }

  function openProject(name) {
    if (steps.dirty || steps.editor || previewHasUnsavedWork()) return;
    clearMarks();
    clearMarks(preview);
    if (!name || name === "desk") {
      goTab("desk");
      return;
    }
    state.threadFilter = null;
    state.selectedProject = name;
    state.focusProject = name;
    state.tab = "project";
    state.searchQuery = "";
    state.searchPinned = false;
    state.peekId = null;
    state.overlay = null;
    state.quickOwner = "outer";
    state.stage = "board";
    state.stageOrigin = null;
    dropProjectPreview();
  }

  function selectedRow() {
    return (
      buildRows().find(
        (row) => row.selectable && row.id === state.selectedId,
      ) || null
    );
  }

  function dropProjectPreview() {
    preview.project = null;
    preview.selectedId = null;
    preview.markMode = false;
    preview.markedIds.clear();
    preview.pendingDelete = null;
    preview.pendingDeleteBulk = false;
    preview.threadFilter = null;
    preview.peekId = null;
    preview.drawer = false;
    preview.archivedOpen = false;
    preview.page = false;
    preview.editField = null;
    preview.editDraft = "";
    preview.undo = null;
    preview.lastClick = 0;
    preview.message = "";
    preview.searchQuery = "";
    preview.searchPinned = false;
    previewSteps.reset();
  }

  function bindProjectPreview() {
    if (!projectsOverview()) return false;
    const row = selectedRow();
    const project = row?.kind === "project" ? row.project : null;
    if (!project) {
      dropProjectPreview();
      return false;
    }
    if (preview.project === project) {
      ensurePreviewSelection();
      return true;
    }
    if (previewHasUnsavedWork()) {
      preview.message = "save or cancel edits before switching tasks";
      return false;
    }
    dropProjectPreview();
    preview.project = project;
    ensurePreviewSelection();
    return true;
  }

  function openProjectPreview() {
    if (!projectsOverview() || !isWideSplit() || state.stage !== "board")
      return false;
    if (!bindProjectPreview()) return false;
    state.stage = "split";
    return true;
  }

  function moveProjectCursor(delta) {
    const ids = selectableIds(buildRows());
    if (!ids.length) return;
    let i = ids.indexOf(state.selectedId);
    if (i < 0) i = 0;
    const next = ids[(i + delta + ids.length) % ids.length];
    const row = buildRows().find((item) => item.id === next);
    if (row?.kind !== "project") return;
    if (
      projectsPreviewActive() &&
      preview.project !== row.project &&
      !bindProjectPreviewFor(row.project)
    )
      return;
    state.selectedId = next;
    if (isWideSplit()) {
      if (state.stage === "board") openProjectPreview();
      else if (projectsPreviewActive()) bindProjectPreview();
    }
  }

  function bindProjectPreviewFor(project) {
    if (preview.project === project) return true;
    if (previewHasUnsavedWork()) {
      preview.message = "save or cancel edits before switching tasks";
      return false;
    }
    const row = buildRows().find(
      (item) => item.kind === "project" && item.project === project,
    );
    if (!row) return false;
    dropProjectPreview();
    preview.project = project;
    ensurePreviewSelection();
    return true;
  }

  function toggleAllGroups() {
    clearMarks();
    if (!state.drawer && (state.focusProject || state.tab !== "projects")) {
      state.inboxOpen = !state.inboxOpen;
      return;
    }
    if (state.focusProject || state.tab !== "projects") return;
    const groups = buildRows().filter(
      (row) => row.kind === "group" && !row.indent,
    );
    const collapse = groups.some(
      (row) => !state.collapsed.has(row.collapseKey),
    );
    for (const row of groups) {
      if (collapse) state.collapsed.add(row.collapseKey);
      else state.collapsed.delete(row.collapseKey);
    }
  }

  function resetDemo() {
    state.tasks = seed();
    state.threadFilter = null;
    state.projectView = null;
    state.filterI = 0;
    state.tab = "desk";
    state.selectedProject = "launchpad";
    state.focusProject = null;
    state.searchQuery = "";
    state.searchPinned = false;
    state.collapsed = new Set();
    state.selectedId = "t1";
    state.markMode = false;
    state.markedIds = new Set();
    state.pendingDelete = null;
    state.pendingDeleteBulk = false;
    state.message = "";
    state.peekId = null;
    state.drawer = false;
    state.archivedOpen = false;
    state.inboxOpen = true;
    state.overlay = null;
    state.draft = "";
    state.refuse = "";
    state.quickExpanded = false;
    state.capture = null;
    state.quickStage = null;
    state.quickOwner = "outer";
    state.copyNotice = "";
    state.undo = null;
    state.stage = "board";
    state.stageOrigin = null;
    dropProjectPreview();
  }

  function openQuickAdd(owner = "outer") {
    state.overlay = "quick";
    state.quickOwner = owner;
    state.draft = "";
    state.refuse = "";
    state.quickExpanded = false;
    state.capture = null;
    state.quickStage = null;
  }

  function captureDirectives(raw) {
    const parts = String(raw).trim().split(/\s+/).filter(Boolean);
    const directives = [];
    for (let i = 0; i < parts.length; i += 1) {
      if (parts[i] !== "!p" && parts[i] !== "!t") continue;
      directives.push(parts[i]);
      if (parts[i + 1] && !parts[i + 1].startsWith("!")) {
        directives.push(parts[i + 1]);
        i += 1;
      }
    }
    return directives;
  }

  function expandQuickAdd() {
    const parsed = parseCapture(state.draft, fallbackProject());
    const now = clock();
    state.capture = {
      id: "capture-draft",
      number: state.nextNumber,
      title: parsed.title,
      notes: "",
      steps: [],
      thread: parsed.thread === undefined ? null : parsed.thread,
      project: parsed.project,
      status: "open",
      createdAt: now,
      updatedAt: now,
      rawDraft: state.draft,
    };
    state.quickExpanded = true;
    state.refuse = "";
    if (state.quickOwner === "preview") {
      preview.page = true;
      preview.editField = "notes";
      preview.editDraft = "";
      previewSteps.bind(state.capture);
    } else {
      state.quickStage = state.stage;
      state.stage = "page";
      state.editField = "notes";
      state.editDraft = "";
      steps.bind(state.capture);
    }
  }

  function collapseQuickAdd() {
    const capture = state.capture;
    if (capture) {
      persistPageText(
        state.quickOwner === "preview" ? preview : state,
        capture,
      );
      state.draft = [capture.title, ...captureDirectives(capture.rawDraft)]
        .filter(Boolean)
        .join(" ");
    }
    state.quickExpanded = false;
    state.capture = null;
    state.refuse = "";
    if (state.quickOwner === "preview") {
      preview.page = false;
      preview.editField = null;
      preview.editDraft = "";
      previewSteps.reset();
    } else {
      state.stage = state.quickStage || "board";
      state.editField = null;
      state.editDraft = "";
      steps.reset();
    }
    state.quickStage = null;
  }

  function saveExpandedDraft() {
    const capture = state.capture;
    if (!capture) return false;
    persistPageText(state.quickOwner === "preview" ? preview : state, capture);
    const title = capture.title.trim();
    if (!title) {
      state.refuse = "title needed";
      return false;
    }
    const owner = state.quickOwner;
    const id = `n${state.nextId++}`;
    const now = clock();
    const task = {
      ...capture,
      id,
      number: state.nextNumber++,
      title,
      steps: structuredClone(capture.steps),
      createdAt: now,
      updatedAt: now,
    };
    delete task.rawDraft;
    state.tasks.unshift(task);
    state.quickExpanded = false;
    state.capture = null;
    state.overlay = null;
    state.quickOwner = "outer";
    state.quickStage = null;
    state.refuse = "";
    if (owner === "preview") {
      preview.selectedId = id;
      preview.editField = null;
      preview.editDraft = "";
      previewSteps.reset();
      preview.page = false;
    } else {
      state.selectedId = id;
      state.editField = null;
      state.editDraft = "";
      steps.reset();
      state.stage = "page";
    }
    return true;
  }

  function hintBar(items) {
    return items
      .map(
        (item) =>
          `<button type="button" class="tsk-verb" data-hint="${esc(item.id)}">${esc(item.label)}</button>`,
      )
      .join("<span> · </span>");
  }

  function runHint(id) {
    const add = document.getElementById("tsk-add");
    if (add) state.draft = add.value;
    if (state.overlay === "quick" && !state.quickExpanded) {
      if (id === "save") saveDraft(false);
      else if (id === "details") expandQuickAdd();
      else if (id === "close") {
        state.overlay = null;
        state.quickOwner = "outer";
        state.refuse = "";
        frame.focus();
      } else return false;
      return true;
    }
    if (state.overlay === "quick" && state.quickExpanded) {
      if (id === "save-expanded") saveExpandedDraft();
      else if (id === "next-field")
        movePageFocus(false, state.quickOwner === "preview");
      else if (id === "collapse") collapseQuickAdd();
      else return false;
      return true;
    }
    if (state.overlay === "search") {
      const owner = searchOwner();
      if (id === "pin") {
        pinSearch(owner);
        state.overlay = null;
      } else if (id === "close") {
        updateSearchQuery(owner, "");
        owner.searchPinned = false;
        state.overlay = null;
      } else return false;
      return true;
    }
    return false;
  }

  function saveDraft(stay) {
    const owner = state.quickOwner === "preview" ? "preview" : "outer";
    const parsed = parseCapture(state.draft, fallbackProject());
    if (!parsed.title) {
      state.refuse = "title needed";
      return false;
    }
    const id = `n${state.nextId++}`;
    const now = Date.now();
    const task = {
      id,
      number: state.nextNumber++,
      title: parsed.title,
      notes: "",
      status: "open",
      project: parsed.project,
      thread: parsed.thread === undefined ? null : parsed.thread,
      createdAt: now,
      updatedAt: now,
    };
    state.tasks.unshift(task);
    if (owner === "preview") {
      preview.selectedId = id;
      preview.peekId = null;
      preview.message = "";
    } else {
      state.selectedId = id;
    }
    state.flashId = id;
    state.refuse = "";
    state.draft = "";
    if (!stay) {
      state.overlay = null;
      state.quickOwner = "outer";
    }
    setTimeout(() => {
      if (state.flashId === id) {
        state.flashId = null;
        render();
      }
    }, 420);
    return true;
  }

  function fileSelected() {
    const tasks = targetTasks();
    for (const task of tasks) task.archived = !task.archived;
    clearMarks();
    state.pendingDelete = null;
    state.message = "";
    state.peekId = null;
    // The file verb has no undo entry: ctrl+u never brings it back.
  }

  function deleteSelected() {
    const tasks = state.pendingDelete
      ? state.pendingDelete.map(taskById).filter(Boolean)
      : targetTasks();
    if (!tasks.length) return;
    const bulk = state.pendingDelete
      ? state.pendingDeleteBulk
      : state.markedIds.size > 0;
    const ids = tasks.map((task) => task.id).sort();
    const signature = ids.join("\n");
    if (
      !state.pendingDelete ||
      state.pendingDelete.slice().sort().join("\n") !== signature
    ) {
      state.pendingDelete = ids;
      state.pendingDeleteBulk = bulk;
      clearMarks();
      state.message = bulk
        ? `press x again to delete ${tasks.length} ${tasks.length === 1 ? "task" : "tasks"}`
        : "press x again to delete";
      return;
    }
    rememberUndo(state, tasks);
    const idSet = new Set(ids);
    state.tasks = state.tasks.filter((task) => !idSet.has(task.id));
    state.pendingDelete = null;
    state.pendingDeleteBulk = false;
    clearMarks();
    state.message = bulk
      ? `deleted ${tasks.length} ${tasks.length === 1 ? "task" : "tasks"} · u restores`
      : `Deleted "${tasks[0].title}" · u undo`;
    state.peekId = null;
  }

  function undoDelete() {
    clearMarks();
    state.pendingDelete = null;
    if (!state.undo) return;
    for (const { task, index } of state.undo.items
      .slice()
      .sort((a, b) => a.index - b.index)) {
      const live = taskById(task.id);
      if (live) Object.assign(live, task);
      else state.tasks.splice(Math.min(index, state.tasks.length), 0, task);
    }
    state.selectedId = state.undo.items[0]?.task.id || state.selectedId;
    state.undo = null;
    state.message = "";
  }

  function toggleBlock() {
    const tasks = targetTasks();
    if (!state.markedIds.size && tasks[0]?.status === "done") return;
    const status =
      tasks.length && tasks.every((task) => task.status === "blocked")
        ? "ready"
        : "blocked";
    setStatus(status);
  }

  function toggleReview() {
    const tasks = targetTasks();
    if (!state.markedIds.size && tasks[0]?.status === "done") return;
    const status =
      tasks.length && tasks.every((task) => task.status === "review")
        ? "ready"
        : "review";
    setStatus(status);
  }

  function primaryVerb() {
    const tasks = targetTasks();
    const changed = tasks.filter(
      (task) => task.status === "open" || task.status === "ready",
    );
    for (const task of changed) {
      task.status = "started";
      task.updatedAt = clock();
      task.statusAt = task.updatedAt;
    }
    clearMarks();
    state.pendingDelete = null;
    state.message = "";
    flashTasks(changed);
  }

  function previewSetStatus(status) {
    const tasks = targetTasks(preview);
    const changed = tasks.filter((task) => task.status !== status);
    if (status === "done" && changed.length) rememberUndo(preview, changed);
    for (const task of changed) {
      task.status = status;
      task.updatedAt = clock();
      task.statusAt = task.updatedAt;
    }
    clearMarks(preview);
    preview.pendingDelete = null;
    preview.message = "";
    flashTasks(changed);
  }

  function previewToggleBlock() {
    const tasks = targetTasks(preview);
    if (!preview.markedIds.size && tasks[0]?.status === "done") return;
    previewSetStatus(
      tasks.length && tasks.every((task) => task.status === "blocked")
        ? "ready"
        : "blocked",
    );
  }

  function previewToggleReview() {
    const tasks = targetTasks(preview);
    if (!preview.markedIds.size && tasks[0]?.status === "done") return;
    previewSetStatus(
      tasks.length && tasks.every((task) => task.status === "review")
        ? "ready"
        : "review",
    );
  }

  function previewPrimaryVerb() {
    const tasks = targetTasks(preview);
    const changed = tasks.filter(
      (task) => task.status === "open" || task.status === "ready",
    );
    for (const task of changed) {
      task.status = "started";
      task.updatedAt = clock();
      task.statusAt = task.updatedAt;
    }
    clearMarks(preview);
    preview.pendingDelete = null;
    preview.message = "";
    flashTasks(changed);
  }

  function previewFileSelected() {
    for (const task of targetTasks(preview)) task.archived = !task.archived;
    clearMarks(preview);
    preview.pendingDelete = null;
    preview.message = "";
    preview.peekId = null;
  }

  function previewDeleteSelected() {
    const tasks = preview.pendingDelete
      ? preview.pendingDelete.map(taskById).filter(Boolean)
      : targetTasks(preview);
    if (!tasks.length) return;
    const bulk = preview.pendingDelete
      ? preview.pendingDeleteBulk
      : preview.markedIds.size > 0;
    const ids = tasks.map((task) => task.id).sort();
    if (!preview.pendingDelete) {
      preview.pendingDelete = ids;
      preview.pendingDeleteBulk = bulk;
      clearMarks(preview);
      preview.message = bulk
        ? `press x again to delete ${tasks.length} ${tasks.length === 1 ? "task" : "tasks"}`
        : "press x again to delete";
      return;
    }
    rememberUndo(preview, tasks);
    const idSet = new Set(ids);
    state.tasks = state.tasks.filter((task) => !idSet.has(task.id));
    preview.pendingDelete = null;
    preview.pendingDeleteBulk = false;
    clearMarks(preview);
    preview.message = bulk
      ? `deleted ${tasks.length} ${tasks.length === 1 ? "task" : "tasks"} · u restores`
      : `Deleted "${tasks[0].title}" · u undo`;
    preview.peekId = null;
  }

  function previewUndoDelete() {
    clearMarks(preview);
    preview.pendingDelete = null;
    if (!preview.undo) return;
    for (const { task, index } of preview.undo.items
      .slice()
      .sort((a, b) => a.index - b.index)) {
      const live = taskById(task.id);
      if (live) Object.assign(live, task);
      else state.tasks.splice(Math.min(index, state.tasks.length), 0, task);
    }
    preview.selectedId = preview.undo.items[0]?.task.id || preview.selectedId;
    preview.undo = null;
    preview.message = "";
  }

  function movePreview(delta) {
    if (previewHasUnsavedWork()) return;
    preview.pendingDelete = null;
    preview.message = "";
    const ids = previewSelectableIds();
    if (!ids.length) return;
    let i = ids.indexOf(preview.selectedId);
    if (i < 0) i = 0;
    preview.selectedId = ids[(i + delta + ids.length) % ids.length];
    preview.peekId = null;
  }

  function openPreviewTaskPage(editField = null) {
    const task = previewTask();
    if (!task) return;
    preview.page = true;
    preview.editField = editField;
    preview.editDraft =
      editField === "title"
        ? task.title
        : editField === "notes"
          ? task.notes || ""
          : "";
    previewSteps.bind(task);
  }

  function leavePreviewTaskPage() {
    previewSteps.reset();
    preview.page = false;
    preview.editField = null;
    preview.editDraft = "";
  }

  function commitPreviewEdit() {
    const task = previewTask();
    if (!task || !preview.editField) return;
    if (preview.editField === "title") {
      const title = preview.editDraft.trim();
      if (title) task.title = title;
    } else {
      task.notes = preview.editDraft;
    }
    task.updatedAt = clock();
    preview.editField = null;
    preview.editDraft = "";
  }

  function renderHelp() {
    const rows = [
      ["navigation", "↑↓ / jk", "move", "select"],
      ["navigation", "shift+M", "multi-select", "select multiple"],
      [
        "navigation",
        "shift+↑↓",
        "mark and move (multi-select)",
        "select multiple",
      ],
      ["navigation", "space", "toggle mark (multi-select)", "select multiple"],
      ["navigation", "enter", "open task", "detail"],
      ["navigation", "←/h · →/l", "close or peek / slide", "wide view"],
      ["task actions", "s", "start", "status open or ready"],
      ["task actions", "d", "done", "status finish complete"],
      ["task actions", "b", "block", "status"],
      ["task actions", "r", "review", "status"],
      ["task actions", "n", "pick", "status ready"],
      ["task actions", "o", "inbox", "status open"],
      ["task actions", "x", "delete", "remove"],
      ["task actions", "u", "undo", "restore"],
      ["task actions", "f", "archive", "file hide"],
      ["create & edit", "+", "quick-add", "capture new task"],
      ["create & edit", "e", "edit title", "rename"],
      ["create & edit", "a", "add step (task page)", "checklist"],
      ["views & find", "1", "desk", "switch view"],
      ["views & find", "2", "selected project", "switch view"],
      ["views & find", "3", "projects", "switch view"],
      ["views & find", "p", "project picker", "switch find"],
      ["views & find", "z / D", "done drawer", "completed tasks"],
      ["views & find", "g", "toggle inbox / groups", "collapse expand"],
      ["views & find", "/", "search board", "find filter tasks projects"],
      ["views & find", ":", "command palette", "find actions"],
      ["app controls", "?", "help", "shortcuts keys"],
      ["app controls", "esc", "clear / close", "cancel"],
      ["app controls", "q", "quit", "exit"],
    ];
    const query = state.helpQ.trim().toLowerCase();
    const visible = rows.filter(
      (row) => !query || row.join(" ").toLowerCase().includes(query),
    );
    let previous = "";
    const list = visible
      .map((row) => {
        const heading =
          row[0] === previous
            ? ""
            : `<div class="tsk-help-group">${esc(row[0].toUpperCase())}</div>`;
        previous = row[0];
        return `${heading}<div class="tsk-help-row"><span>${esc(row[1])}</span><span>${esc(row[2])}</span></div>`;
      })
      .join("");
    return `
      <div class="tsk-box tsk-help" role="dialog" aria-label="help">
        <div class="tsk-box-top"><span class="tsk-box-title">help</span><button type="button" class="tsk-box-close" data-close="1" aria-label="close">[x]</button></div>
        <div class="tsk-help-search"><span>/ </span><span>${state.helpQ ? esc(state.helpQ) : '<span class="dim">search keys or actions…</span>'}</span><span class="cursor">█</span></div>
        <div class="tsk-help-divider" aria-hidden="true"></div>
        <div class="tsk-box-body tsk-help-body">${list || '<div class="dim">no shortcuts match</div>'}</div>
        <div class="tsk-box-foot">type search · ↑/↓ scroll · esc clear/close</div>
      </div>`;
  }

  function renderPalette() {
    const cmds = paletteCommands();
    if (state.paletteI >= cmds.length)
      state.paletteI = Math.max(0, cmds.length - 1);
    const list = cmds
      .map((c, i) => {
        const mark = i === state.paletteI ? "▸" : " ";
        const cls = i === state.paletteI ? "sel-text" : "";
        return `<div class="tsk-pal-row ${cls}" data-cmd="${esc(c.id)}">${mark} ${esc(c.label)}</div>`;
      })
      .join("");
    return `
      <div class="tsk-box tsk-palette" role="dialog" aria-label="command">
        <div class="tsk-box-top"><span class="tsk-box-title">command</span><button type="button" class="tsk-box-close" data-close="1" aria-label="close">[x]</button></div>
        <div class="tsk-box-body">${list || `<div class="dim">  no matches</div>`}</div>
        <div class="tsk-box-foot">↑/↓ move · enter run · esc close · type to filter</div>
      </div>
      <div class="tsk-input-row"><span class="tsk-prompt">:</span><span class="tsk-draft">${esc(state.paletteQ)}</span><span class="cursor">█</span></div>`;
  }

  function renderPicker() {
    const opts = pickerOptions();
    if (state.pickerI >= opts.length)
      state.pickerI = Math.max(0, opts.length - 1);
    const list = opts
      .map((name, i) => {
        const mark = i === state.pickerI ? "▸" : " ";
        const cls = i === state.pickerI ? "sel-text" : "";
        return `<div class="tsk-pal-row" data-pick="${esc(name)}"><span class="${cls}">${mark} ${esc(name)}</span></div>`;
      })
      .join("");
    return `
      <div class="tsk-box tsk-palette" role="dialog" aria-label="project">
        <div class="tsk-box-top"><span class="tsk-box-title">project</span><button type="button" class="tsk-box-close" data-close="1" aria-label="close">[x]</button></div>
        <div class="tsk-box-body">${list}</div>
        <div class="tsk-box-foot">↑/↓ move · enter choose · esc close</div>
      </div>`;
  }

  function runPageVerb(id) {
    if (id === "close") {
      state.overlay = null;
      state.editField = null;
      state.editDraft = "";
      leaveTaskPage();
      return;
    }
    clearMarks();
    enterTaskStage();
    const task = selectedTask();
    if (!task) return;
    if (id === "edit" && steps.selected && steps.selected !== "add") {
      steps.begin(task, steps.selected);
      return;
    }
    if (id === "edit") {
      state.editField = "title";
      state.editDraft = task.title;
    }
    if (id === "notes") {
      state.editField = "notes";
      state.editDraft = task.notes || "";
    }
    if (id === "done") setStatus("done");
    if (id === "block") toggleBlock();
    if (id === "file") fileSelected();
  }

  function runPreviewPageVerb(id) {
    if (id === "close") {
      leavePreviewTaskPage();
      return;
    }
    clearMarks(preview);
    const task = previewTask();
    if (!task) return;
    if (
      id === "edit" &&
      previewSteps.selected &&
      previewSteps.selected !== "add"
    ) {
      previewSteps.begin(task, previewSteps.selected);
      return;
    }
    if (id === "edit") openPreviewTaskPage("title");
    if (id === "done") previewSetStatus("done");
    if (id === "block") previewToggleBlock();
  }

  function runPreviewVerb(id) {
    if (id === "open") openPreviewTaskPage();
    if (id === "capture") openQuickAdd("preview");
    if (id === "start") previewPrimaryVerb();
    if (id === "done") previewSetStatus("done");
    if (id === "block") previewToggleBlock();
    if (id === "reopen" && previewTask()?.status === "done")
      previewSetStatus("ready");
    if (id === "file") previewFileSelected();
    if (id === "delete") previewDeleteSelected();
    if (id === "help") {
      state.overlay = "help";
      state.helpQ = "";
    }
  }

  // The wide task column: a header rule on the selector row (dim in split, bold when the task
  // owns focus) and the page body under it. Narrow: the same page fills the frame.
  function taskColumnWidth() {
    const cols = terminalColumns();
    return state.stage === "split"
      ? cols - Math.floor(cols * 0.4) - 2
      : state.stage === "rail"
        ? cols - 34
        : cols;
  }

  function renderTaskPage(task, { preview: previewMode, embedded }) {
    const pageSteps = previewMode ? previewSteps : steps;
    const editing = previewMode ? preview.editField : state.editField;
    const editDraft = previewMode ? preview.editDraft : state.editDraft;
    const focused = previewMode || taskFocus();
    const narrow = !previewMode && !isWideSplit();
    const editId = previewMode ? "tsk-preview-edit" : "tsk-edit";
    const stepEditId = previewMode ? "tsk-preview-step-edit" : "tsk-step-edit";
    const stepAttribute = previewMode ? "data-preview-step" : "data-step";
    const addAttribute = previewMode
      ? "data-preview-step-add"
      : "data-step-add";
    const project = previewMode
      ? preview.project || "project"
      : task
        ? projectName(task)
        : "desk";
    if (!task) {
      if (embedded) {
        return `<div class="tsk-task-column tsk-surface" aria-label="${previewMode ? "project " : ""}task column"><div class="tsk-task-header dim"><span class="sec">no task</span></div><div class="tsk-task-rule" aria-hidden="true"></div><div class="tsk-task-surface"><div class="dim">  select a task to preview it here</div></div></div>`;
      }
      return `<div class="tsk-overlay"><div class="dim">no task</div><div class="dim">  select a task to preview it here</div></div>`;
    }
    pageSteps.bind(task);
    const stateSlot = pageSteps.editor
      ? "editing step"
      : pageSteps.dirty
        ? "unsaved"
        : editing
          ? `editing ${editing}`
          : narrow
            ? task.status
            : `${task.status} · ${project}`;
    const headTitle =
      editing === "title"
        ? `<input class="tsk-field" id="${editId}" value="${esc(editDraft)}" />`
        : esc(task.title);
    const glyph = GLYPH[task.status] || "○";
    let header = `<div class="tsk-task-header ${focused ? "is-bold" : "dim"}"><span class="glyph">${glyph}</span> <span class="tsk-task-id" data-copy-task="${esc(task.id)}" title="copy T${task.number}">T${task.number}</span> <span class="sec">${headTitle}</span><span class="tsk-state-slot">${esc(stateSlot)}</span></div><div class="tsk-task-rule" aria-hidden="true"></div>`;
    if (narrow && !editing) {
      const room = terminalColumns() - 9 - stateSlot.length;
      const titleRows = wrapText(task.title, room);
      const headerStatus =
        titleRows[0].length + 8 + stateSlot.length <= terminalColumns() - 2
          ? stateSlot
          : "";
      header = `<div class="tsk-task-header tsk-narrow-header"><span class="tsk-page-prefix">${glyph} <span class="tsk-task-id" data-copy-task="${esc(task.id)}">T${task.number}</span> </span><span class="tsk-page-title">${titleRows.map((line) => `<span>${esc(line)}</span>`).join("")}</span><span class="tsk-state-slot">${esc(headerStatus)}</span></div><div class="tsk-task-rule"></div>`;
    }
    const notes =
      editing === "notes"
        ? `<textarea class="tsk-field tsk-notes" id="${editId}">${esc(editDraft)}</textarea>`
        : `<div class="tsk-page-notes">${wrapText(
            task.notes || "no notes yet",
            taskColumnWidth() - 6,
          )
            .map((line) => `<span>${esc(line) || " "}</span>`)
            .join("")}</div>`;
    const stepRows = pageSteps.rows(task);
    const inlineEditor = `<textarea id="${stepEditId}" class="tsk-field" aria-label="Step text" rows="${wrapText(pageSteps.editor?.text ?? "", taskColumnWidth() - 8).length}">${esc(pageSteps.editor?.text ?? "")}</textarea><span class="tsk-step-refusal">${esc(pageSteps.refusal)}</span>`;
    const stepList = `<div class="tsk-steps"><div class="tsk-steps-heading dim">steps ${stepRows.filter((step) => step.done).length}/${stepRows.length}</div>${stepRows
      .map(
        (step) =>
          `<div class="tsk-step" ${stepAttribute}="${esc(step.id)}" role="option" aria-selected="${pageSteps.selected === step.id}"><span class="tsk-step-glyph">${pageSteps.selected === step.id ? "▸ " : "  "}${pageSteps.marked === step.id ? "✗" : step.done ? "✓" : "▪"} </span>${
            pageSteps.editor?.id === step.id
              ? inlineEditor
              : `<span class="tsk-step-text">${wrapText(
                  step.text,
                  narrow
                    ? terminalColumns() - 8
                    : Math.max(8, taskColumnWidth() - 8),
                )
                  .map((line) => `<span>${esc(line)}</span>`)
                  .join("")}</span>`
          }</div>`,
      )
      .join(
        "",
      )}${pageSteps.editor && !pageSteps.editor.id ? `<div class="tsk-step-new">${inlineEditor}</div>` : `<button type="button" class="tsk-step-add dim" ${addAttribute}="1">   + step</button>`}</div>`;
    const metaField = (field, text) =>
      editing === field
        ? `<span class="tsk-meta-selected" data-page-field="${field}">${text}</span>`
        : text;
    const thread = task.thread
      ? `#${esc(task.thread)}`
      : editing === "thread"
        ? "thread"
        : "";
    const scope = previewMode || narrow || editing ? esc(project) : "";
    const parts = [
      thread && metaField("thread", thread),
      scope && metaField("scope", scope),
      `created ${esc(age(task.createdAt))} ago`,
      `updated ${esc(age(task.updatedAt))} ago`,
    ].filter(Boolean);
    const meta = `<div class="tsk-page-meta dim">${parts.join(" · ")}</div>`;
    const editField = editing?.startsWith("step:") ? "steps" : editing || "";
    const editTarget = editing?.startsWith("step:")
      ? editing.slice("step:".length)
      : "";
    return `<div class="tsk-task-column tsk-surface ${narrow ? "is-narrow" : ""}" aria-label="T${task.number}${previewMode ? " project" : ""} task column" data-status="${esc(task.status)}" data-edit-state="${pageSteps.editor ? "editing" : pageSteps.dirty ? "unsaved" : "view"}" data-edit-field="${esc(editField)}" data-edit-target="${esc(editTarget)}">${header}<div class="tsk-task-surface tsk-page">${notes}${stepList}</div>${meta}</div>`;
  }

  function renderPage(embedded = false) {
    return renderTaskPage(selectedTask(), { preview: false, embedded });
  }

  function previewPageVerbBar() {
    if (previewSteps.editor || previewSteps.dirty || preview.editField)
      return "shift+enter save · esc cancel";
    return PAGE_VERBS.map(
      (verb) =>
        `<button type="button" class="tsk-verb" data-preview-page-verb="${verb.id}">${verb.label}</button>`,
    ).join("<span> · </span>");
  }

  function previewBoardWidth() {
    const columns = terminalColumns();
    return Math.max(
      8,
      (state.stage === "split"
        ? columns - Math.floor(columns * 0.4) - 3
        : columns - 34) - 4,
    );
  }

  function renderPreviewPage() {
    return renderTaskPage(previewTask(), { preview: true, embedded: true });
  }

  function renderPreviewBoard(rail) {
    ensurePreviewSelection();
    const width = previewBoardWidth();
    const previewRowsNow = previewRows();
    const body = previewRowsNow
      .map((row) => {
        if (row.kind === "section")
          return `<div class="tsk-sec"><span class="sec">${esc(row.label)}</span><span class="rule" aria-hidden="true"></span><span class="count">${row.count}</span></div>`;
        if (row.kind === "archived") {
          const mark = preview.archivedOpen ? "▾" : "▸";
          const selected = row.id === preview.selectedId;
          return `<button type="button" class="tsk-group" data-preview-archived="1"><span class="dim">${mark}</span> <span class="sec">${selected ? "<strong>archived</strong>" : "archived"}</span><span class="rule" aria-hidden="true"></span><span class="count dim">${row.count}</span></button>`;
        }
        const task = row.task;
        const selected = task.id === preview.selectedId;
        const marked = preview.markedIds.has(task.id);
        const rowMark = selected
          ? marked
            ? "▸▪"
            : "▸ "
          : marked
            ? "▪ "
            : "  ";
        const flash = task.id === state.flashId;
        const glyph = GLYPH[task.status] || "○";
        const titleLines = wrapText(
          task.title,
          Math.max(8, width - 2 - 4 - `T${task.number} `.length),
        );
        const title = titleLines
          .map((line) => `<span class="tsk-title-line">${esc(line)}</span>`)
          .join("");
        const noteLines = wrapText(
          (task.notes || "").trim() || "no notes yet",
          Math.max(8, width - 7),
        );
        const peek =
          rail && preview.peekId === task.id
            ? [
                ...noteLines
                  .slice(0, 5)
                  .map(
                    (line) =>
                      `<div class="tsk-peek dim">    │ ${esc(line)}</div>`,
                  ),
                ...(noteLines.length > 5
                  ? [
                      `<div class="tsk-peek dim">    │ … ${noteLines.length - 5} more lines</div>`,
                    ]
                  : []),
                ...(task.thread
                  ? [
                      `<div class="tsk-attribution dim">    └─ #${esc(task.thread)}</div>`,
                    ]
                  : [`<div class="tsk-peek dim">    └</div>`]),
              ].join("")
            : "";
        return `<button type="button" class="tsk-row ${row.dim ? "dim" : ""} ${selected ? "is-sel" : ""} ${flash ? "is-flash" : ""}" data-preview-task="${esc(task.id)}"><span class="tsk-row-main"><span class="tsk-row-prefix">${rowMark}<span class="tsk-row-glyph">${glyph}</span> <span class="tsk-task-id" data-copy-task="${esc(task.id)}" title="copy T${task.number}">T${task.number}</span> </span><span class="tsk-row-title">${title}</span></span></button>${peek}`;
      })
      .join("");
    const project = esc(preview.project || "project");
    const filter = `<button type="button" class="tsk-view-control dim" data-preview-filter="1">${esc(previewFilterLabel())} ▾</button>`;
    const empty = preview.searchQuery.trim()
      ? `no tasks match &quot;${esc(preview.searchQuery.trim())}&quot;`
      : "nothing here";
    return `<div class="tsk-project-preview tsk-surface ${rail ? "is-live" : "is-preview"}" aria-label="${project} project preview"><div class="tsk-project-preview-header ${rail ? "is-bold" : "dim"}"><span class="sec">${project}</span>${rail ? filter : ""}</div><div class="tsk-task-rule" aria-hidden="true"></div><div class="tsk-list">${body || `<div class="dim">  ${empty}</div>`}</div></div>`;
  }

  function stageHint() {
    if (!isWideSplit()) return "";
    if (projectsOverview()) {
      if (state.stage === "board") return "→ project pane";
      if (state.stage === "split")
        return "index ▸ project    → project · ← close";
      if (projectsPreviewPage()) return "project task page    ← close";
      return previewHasUnsavedWork()
        ? "index ◂ project"
        : "index ◂ project    ← index";
    }
    if (state.editField) return "shift+enter save · esc cancel";
    if (state.stage === "board") return "→ task pane";
    if (state.stage === "split")
      return "board ▸ task    → task · ← close · enter open";
    if (state.stage === "rail") return "board ◂ task    ← board · → full page";
    return "← rail";
  }

  // Same overflow rule as the native threads_cell: reserve room for hidden names.
  function threadCell(threads, width) {
    let out = "",
      shown = 0;
    for (const thread of threads) {
      const candidate = out ? out + " #" + thread : "#" + thread;
      const remaining = threads.length - shown - 1;
      const suffix = remaining ? "  +" + remaining : "";
      if (candidate.length + suffix.length > width) break;
      out = candidate;
      shown++;
    }
    if (!shown)
      return threads.length ? ("+" + threads.length).slice(0, width) : "";
    return (
      out + (shown < threads.length ? "  +" + (threads.length - shown) : "")
    );
  }

  function renderBoard(rows, rail = false, bare = false) {
    const tabs = TABS.map(([tab, label]) => {
      const on = state.tab === tab;
      const text = tab === "project" ? state.selectedProject : label;
      return `<span class="tsk-tab-group ${on ? "is-on" : ""}"><button type="button" class="tsk-tab ${on ? "is-on" : ""}" data-tab="${tab}">${esc(text)}</button>${tab === "project" ? `<button class="tsk-tab-arrow" data-chip="1" aria-label="choose project">▾</button>` : ""}</span>`;
    }).join(`<span class="dim"> · </span>`);
    const filter = state.focusProject
      ? state.threadFilter === null
        ? "all"
        : state.threadFilter === ""
          ? "without a thread"
          : `#${state.threadFilter}`
      : state.tab === "projects"
        ? state.projectView === null
          ? "Overview"
          : `#${state.projectView}`
        : null;
    const control =
      filter === null
        ? ""
        : `<button class="tsk-view-control dim" data-filter="1">${esc(filter)} ▾</button>`;
    const index =
      state.tab === "projects" &&
      state.projectView === null &&
      !state.focusProject;
    const boardColumns =
      index && isWideSplit() && state.stage === "split"
        ? Math.floor(terminalColumns() * 0.4)
        : index && rail
          ? 32
          : terminalColumns();
    const showThreads = index && boardColumns >= 100;
    const available = boardColumns - (showThreads ? 34 : 0);
    const nameWidth = Math.max(24, Math.floor(available * 0.4));
    const threadWidth = Math.max(0, available - nameWidth - 2);
    const count = (n) => n || "·";
    const body = rows
      .map((row) => {
        if (row.kind === "section" || row.kind === "sub") {
          const cls = row.kind === "sub" ? "tsk-sub" : "tsk-sec";
          return `<div class="${cls}"><span class="sec">${esc(row.label)}</span><span class="rule" aria-hidden="true"></span><span class="count">${row.count}</span></div>`;
        }
        if (row.kind === "project") {
          const selected = row.id === state.selectedId;
          if (rail) {
            return `<button type="button" class="tsk-row tsk-rail-project-row ${selected ? "is-sel" : ""}" data-project-row="${esc(row.project)}" data-nav-id="${esc(row.id)}"><span class="tsk-row-main"><span class="tsk-rail-project-name">${selected ? "▸" : " "} ${esc(row.label)}</span><span class="dim tsk-rail-project-counts">${count(row.needs)} ${count(row.motion)} ${count(row.onDeck)} ${count(row.done)}</span></span></button>`;
          }
          const threads = [
            ...new Set(
              state.tasks
                .filter(
                  (t) =>
                    t.project === row.project &&
                    !t.archived &&
                    t.status !== "done",
                )
                .map((t) => t.thread)
                .filter(Boolean),
            ),
          ].sort();
          return `<button type="button" class="tsk-project-row ${selected ? "is-selected" : ""}" data-project-row="${esc(row.project)}" data-nav-id="${esc(row.id)}"><span class="tsk-project-name">${selected ? "▸" : " "} <span>${esc(row.label)}</span>${row.project === launchProject() ? `<span class="dim"> · here</span>` : ""}</span>${showThreads ? `<span class="dim tsk-project-threads">${esc(threadCell(threads, threadWidth))}</span>` : ""}<span class="${row.needs ? "is-bold" : "dim"}">${count(row.needs)}</span><span>${count(row.motion)}</span><span class="dim">${count(row.onDeck)}</span><span class="dim">${count(row.done)}</span></button>`;
        }
        if (row.kind === "group") {
          const mark = row.collapsed ? "▸" : "▾";
          const pad = row.indent ? "  " : "";
          return `<button type="button" class="tsk-group" data-collapse="${esc(row.collapseKey)}" data-project="${esc(row.project || "")}">${pad}<span class="dim">${mark}</span> <span class="sec">${esc(row.label)}</span> <span class="count">${row.count}</span></button>`;
        }
        if (row.kind === "inbox") {
          const mark = state.inboxOpen ? "▾" : "▸";
          const selected = row.id === state.selectedId;
          return `<button type="button" class="tsk-group tsk-inbox" data-inbox-header="1"><span>${mark}</span> <span class="sec">${selected ? "<strong>inbox</strong>" : "inbox"}</span> · <span class="count">${row.count}</span></button>`;
        }
        if (row.kind === "archived") {
          const mark = state.archivedOpen ? "▾" : "▸";
          const selected = row.id === state.selectedId;
          return `<button type="button" class="tsk-group" data-archived-header="1"><span class="dim">${mark}</span> <span class="sec">${selected ? "<strong>archived</strong>" : "archived"}</span><span class="rule" aria-hidden="true"></span><span class="count dim">${row.count}</span></button>`;
        }
        const task = row.task;
        if (rail && task.status === "done") return "";
        const selected = task.id === state.selectedId;
        const marked = state.markedIds.has(task.id);
        const rowMark = selected
          ? marked
            ? "▸▪"
            : "▸ "
          : marked
            ? "▪ "
            : "  ";
        const flash = task.id === state.flashId;
        const glyph = GLYPH[task.status] || "○";
        const indent = "  ".repeat(row.indent || 0);
        if (rail) {
          const prefix = `${rowMark}${glyph} `;
          const lines = wrapText(
            task.title,
            32 - 1 - 4 - `T${task.number} `.length,
          );
          return `<button type="button" class="tsk-row tsk-rail-row" data-task="${task.id}"><span class="tsk-row-main">${prefix}<span class="tsk-task-id" data-copy-task="${esc(task.id)}">T${task.number}</span> ${esc(lines[0])}${lines
            .slice(1)
            .map(
              (line) =>
                `<span class="tsk-rail-continuation">    ${esc(line)}</span>`,
            )
            .join("")}</span></button>`;
        }
        const columns =
          isWideSplit() && state.stage === "split"
            ? Math.floor(terminalColumns() * 0.4)
            : terminalColumns();
        const titleLines = wrapText(
          task.title,
          columns - 2 - 4 - `T${task.number} `.length,
        );
        const title = titleLines
          .map((line) => `<span class="tsk-title-line">${esc(line)}</span>`)
          .join("");
        const noteLines = wrapText(
          (task.notes || "").trim() || "no notes yet",
          columns - 7,
        );
        const label = metaFor(task);
        const peek =
          state.peekId === task.id && !isWideSplit()
            ? [
                ...noteLines
                  .slice(0, 5)
                  .map(
                    (line) =>
                      `<div class="tsk-peek dim">    │ ${esc(line)}</div>`,
                  ),
                ...(noteLines.length > 5
                  ? [
                      `<div class="tsk-peek dim">    │ … ${noteLines.length - 5} more lines</div>`,
                    ]
                  : []),
                ...(label
                  ? wrapText(label, columns - 9).map(
                      (line, i) =>
                        `<div class="tsk-attribution dim">${i ? "       " : "    └─ "}${esc(line)}</div>`,
                    )
                  : [`<div class="tsk-peek dim">    └</div>`]),
              ].join("")
            : "";
        const dimRow = row.dim ? "dim" : "";
        return `<button type="button" class="tsk-row ${dimRow} ${selected ? "is-sel" : ""} ${flash ? "is-flash" : ""}" data-task="${task.id}"><span class="tsk-row-main"><span class="tsk-row-prefix">${indent}${rowMark}<span class="tsk-row-glyph">${glyph}</span> <span class="tsk-task-id" data-copy-task="${esc(task.id)}" title="copy T${task.number}">T${task.number}</span> </span><span class="tsk-row-title">${title}</span></span></button>${peek}`;
      })
      .join("");

    const empty = index
      ? "no projects match"
      : state.searchQuery.trim()
        ? `no tasks match &quot;${esc(state.searchQuery.trim())}&quot;`
        : "nothing here";
    const column = `
      <div class="tsk-tabs">${tabs}${control}</div>
      <div class="tsk-list ${index ? "tsk-project-table" : ""} ${showThreads ? "with-threads" : ""}" style="--project-name-width:${nameWidth + 2}ch;--project-thread-width:${threadWidth + 2}ch">${index ? `<div class="tsk-project-legend"><span>  PROJECT</span>${showThreads ? "<span>THREADS</span>" : ""}<span>NEEDS YOU</span><span>IN MOTION</span><span>ON DECK</span><span>DONE</span></div>` : ""}${body || `<div class="dim">  ${empty}</div>`}</div>`;
    // Wide stages paint one shared footer under both columns, so a column omits its own.
    return rail || bare ? column : column + renderFooter();
  }

  const PAGE_VERBS = [
    { id: "edit", label: "e edit" },
    { id: "done", label: "d done" },
    { id: "block", label: "b block" },
    { id: "close", label: "esc close" },
  ];

  function pageVerbBar() {
    // Same guard as previewPageVerbBar: a title or notes editor owns the footer, so the
    // verbs are not clickable while typing.
    if (steps.editor || steps.dirty || state.editField)
      return "shift+enter save · esc cancel";
    return PAGE_VERBS.map(
      (v) =>
        `<button type="button" class="tsk-verb" data-page-verb="${v.id}">${v.label}</button>`,
    ).join("<span> · </span>");
  }

  // One footer for the frame: a rule, the status row (active lens · stage crumb), and the verb
  // bar for whichever side owns focus. Wide stages paint it under both columns, as the app does.
  function renderFooter() {
    const previewOwnsFooter = projectsPreviewFocused() || projectsPreviewPage();
    const owner = previewOwnsFooter ? preview : state;
    const selectedCount = owner.markedIds.size;
    const selectionMessage = owner.markMode
      ? selectedCount
        ? `multi-select · ${selectedCount} selected · esc clears`
        : "multi-select · space/click marks · esc exits"
      : "";
    const baseContext = previewOwnsFooter
      ? preview.project || "project"
      : state.tab === "projects" && state.projectView === null
        ? projectsPreviewActive() && preview.message
          ? preview.message
          : selectedRow()?.project
            ? projectPath(selectedRow().project)
            : "projects"
        : state.tab === "desk"
          ? "desk"
          : state.focusProject
            ? `${state.focusProject}${state.threadFilter ? ` · #${state.threadFilter}` : ""}`
            : state.tab;
    const scopedContext =
      owner.searchPinned && owner.searchQuery.trim()
        ? `${baseContext} · /${owner.searchQuery.trim()}`
        : baseContext;
    const context = owner.message || selectionMessage || scopedContext;
    const task = previewOwnsFooter ? previewTask() : selectedTask();
    const previewVerbs = task
      ? verbItems(task)
      : [
          { id: "capture", label: "+ add" },
          { id: "help", label: "? help" },
        ];
    const verbs = projectsPreviewPage()
      ? previewPageVerbBar()
      : taskFocus()
        ? pageVerbBar()
        : (previewOwnsFooter ? previewVerbs : verbItems(task))
            .map(
              (v) =>
                `<button type="button" class="tsk-verb" data-${previewOwnsFooter ? "preview-" : ""}verb="${esc(v.id)}">${esc(v.label)}</button>`,
            )
            .join("<span> · </span>");
    const footer =
      state.overlay === "quick" && !state.quickExpanded
        ? `<div class="tsk-input-row"><span class="tsk-prompt">+</span><input class="tsk-field" id="tsk-add" value="${esc(state.draft)}" placeholder="title  ·  !p project  ·  !t thread" autocomplete="off" /><span class="cursor">█</span></div>
           <div class="foot dim tsk-verbs">${
             state.refuse
               ? esc(state.refuse)
               : hintBar([
                   { id: "save", label: "enter save" },
                   { id: "details", label: "tab details" },
                   { id: "close", label: "esc close" },
                 ])
           }</div>`
        : state.overlay === "quick"
          ? `<div class="tsk-status-row"><span class="foot">expanded quick-add</span></div><div class="foot dim tsk-verbs">${
              state.refuse
                ? esc(state.refuse)
                : hintBar([
                    { id: "save-expanded", label: "ctrl+enter save" },
                    { id: "next-field", label: "tab next field" },
                    { id: "collapse", label: "esc one-line draft" },
                  ])
            }</div>`
          : state.overlay === "search"
            ? `<div class="tsk-input-row"><span class="tsk-prompt">/</span><input class="tsk-field" id="tsk-project-search" value="${esc(searchOwner().searchQuery)}" placeholder="${projectsOverview() && !projectsPreviewFocused() ? "search projects" : "search tasks"}" autocomplete="off" /><span class="cursor">█</span></div>
             <div class="foot dim tsk-verbs">${hintBar([
               { id: "pin", label: "enter pin" },
               { id: "close", label: "esc clear" },
             ])}</div>`
            : `<div class="tsk-status-row"><button type="button" class="tsk-done-count foot" data-${previewOwnsFooter ? "preview-" : ""}drawer="1">${esc(context)}</button><span class="foot dim tsk-stage-hint">${esc(stageHint())}</span></div>
           <div class="foot dim tsk-verbs">${verbs}</div>
           ${state.copyNotice ? `<div class="foot dim">${esc(state.copyNotice)}</div>` : ""}`;
    return `
      <div class="tsk-foot">
        <div class="foot-rule" aria-hidden="true"></div>
        ${footer}
      </div>`;
  }

  function render() {
    renderColumns = terminalColumns();
    try {
      const rows = buildRows();
      ensureSelection(rows);
      // Preview refusals belong to unresolved work, not the retained seat.
      if (!previewHasUnsavedWork()) preview.message = "";
      const wide = isWideSplit();
      let html;
      if (wide && projectsOverview() && state.stage === "split") {
        html = `<div class="tsk-wide-split is-split" style="grid-template-columns:${Math.floor(terminalColumns() * 0.4)}ch 2ch minmax(0,1fr)">
           <div class="tsk-board-surface tsk-surface">${renderBoard(rows, false, true)}</div>
           <div class="tsk-rule-column dim" aria-hidden="true"></div>
           ${renderPreviewBoard(false)}
         </div>${renderFooter()}`;
      } else if (wide && projectsOverview() && state.stage === "rail") {
        html = `<div class="tsk-wide-split is-rail">
           <div class="tsk-board-surface tsk-rail tsk-surface dim">${renderBoard(rows, true, true)}</div>
           <div class="tsk-rule-column dim" aria-hidden="true"></div>
           ${preview.page ? renderPreviewPage() : renderPreviewBoard(true)}
         </div>${renderFooter()}`;
      } else if (wide && state.stage === "split") {
        html = `<div class="tsk-wide-split is-split" style="grid-template-columns:${Math.floor(terminalColumns() * 0.4)}ch 2ch minmax(0,1fr)">
           <div class="tsk-board-surface tsk-surface">${renderBoard(rows, false, true)}</div>
           <div class="tsk-rule-column dim" aria-hidden="true"></div>
           ${renderPage(true)}
         </div>${renderFooter()}`;
      } else if (wide && state.stage === "rail") {
        html = `<div class="tsk-wide-split is-rail">
           <div class="tsk-board-surface tsk-rail tsk-surface dim">${renderBoard(rows, true)}</div>
           <div class="tsk-rule-column dim" aria-hidden="true"></div>
           ${renderPage(true)}
         </div>${renderFooter()}`;
      } else if (wide && state.stage === "page") {
        html = `<div class="tsk-wide-split is-page">${renderPage(true)}</div>${renderFooter()}`;
      } else if (taskFocus()) {
        html = `<div class="tsk-single-task">${renderPage(true)}</div>${renderFooter()}`;
      } else {
        html = renderBoard(rows);
      }
      if (state.overlay === "help") html += renderHelp();
      if (state.overlay === "palette") html += renderPalette();
      if (state.overlay === "picker") html += renderPicker();
      if (state.overlay === "filter") html += renderFilter();
      if (state.overlay === "preview-filter") html += renderPreviewFilter();
      const oldScroll = root.querySelector(".tsk-task-surface")?.scrollTop ?? 0;
      const activeSearch = document.activeElement?.id === "tsk-project-search";
      const keepKeys =
        document.activeElement === frame ||
        frame.contains(document.activeElement);
      root.innerHTML = html;
      const content = root.querySelector(".tsk-task-surface");
      if (content) content.scrollTop = oldScroll;
      const stepInput = root.querySelector("#tsk-step-edit");
      const previewStepInput = root.querySelector("#tsk-preview-step-edit");
      if (stepInput || previewStepInput) {
        const input = stepInput || previewStepInput;
        const stepState = stepInput ? steps : previewSteps;
        input.addEventListener("input", () => {
          stepState.editor.text = input.value;
          stepState.refusal = "";
          root.querySelector(".tsk-step-refusal").textContent = "";
          input.rows = wrapText(input.value, taskColumnWidth() - 8).length;
        });
        input.focus({ preventScroll: true });
        input.setSelectionRange(input.value.length, input.value.length);
        input.scrollIntoView({ block: "nearest" });
      }
      const add = document.getElementById("tsk-add");
      const edit = document.getElementById("tsk-edit");
      const previewEdit = document.getElementById("tsk-preview-edit");
      const search = document.getElementById("tsk-project-search");
      if (search) {
        search.addEventListener("input", () => {
          const owner = searchOwner();
          updateSearchQuery(owner, search.value);
          render();
        });
      }
      if (!stepInput && !previewStepInput) {
        if (add) {
          add.focus();
          add.selectionStart = add.value.length;
          add.addEventListener("input", () => {
            state.draft = add.value;
            state.refuse = "";
          });
        } else if (edit) {
          edit.focus();
          edit.addEventListener("input", () => {
            state.editDraft = edit.value;
          });
        } else if (previewEdit) {
          previewEdit.focus();
          previewEdit.addEventListener("input", () => {
            preview.editDraft = previewEdit.value;
          });
        } else if (search && activeSearch) {
          search.focus();
          search.selectionStart = search.value.length;
          search.selectionEnd = search.value.length;
        } else if (keepKeys) {
          frame.focus({ preventScroll: true });
        }
      }
      frame.classList.toggle(
        "is-focused",
        document.activeElement === frame ||
          frame.contains(document.activeElement),
      );
    } finally {
      renderColumns = null;
    }
  }

  function move(delta) {
    state.pendingDelete = null;
    state.message = "";
    if (projectsOverview()) {
      moveProjectCursor(delta);
      return;
    }
    if (steps.dirty || steps.editor) return;
    const ids = selectableIds(buildRows());
    if (!ids.length) return;
    let i = ids.indexOf(state.selectedId);
    if (i < 0) i = 0;
    i = (i + delta + ids.length) % ids.length;
    state.selectedId = ids[i];
    state.peekId =
      state.peekId && state.peekId === state.selectedId ? state.peekId : null;
  }

  function commitEdit() {
    const task = selectedTask();
    if (!task || !state.editField) return;
    if (state.editField === "title") {
      const title = state.editDraft.trim();
      if (title) task.title = title;
    } else if (state.editField === "notes") {
      task.notes = state.editDraft;
    }
    task.updatedAt = clock();
    state.editField = null;
    state.editDraft = "";
  }

  function persistPageText(form, task) {
    if (!task) return;
    if (form.editField === "title") {
      const title = form.editDraft.trim();
      if (title) task.title = title;
    }
    if (form.editField === "notes") task.notes = form.editDraft;
  }

  // The same reversible ring drives full pages, preview pages, and expanded capture:
  // Title → Notes → each step → + step → Thread → Scope → Title.
  function movePageFocus(reverse = false, previewMode = false) {
    const form = previewMode ? preview : state;
    const task = previewMode ? previewTask() : selectedTask();
    const pageSteps = previewMode ? previewSteps : steps;
    if (!task || !form.editField) return;
    persistPageText(form, task);
    const stops = [
      "title",
      "notes",
      ...pageSteps.rows(task).map((step) => `step:${step.id}`),
      "step:add",
      "thread",
      "scope",
    ];
    let index = stops.indexOf(form.editField);
    if (index < 0) index = 0;
    index = (index + (reverse ? -1 : 1) + stops.length) % stops.length;
    form.editField = stops[index];
    form.editDraft =
      form.editField === "title"
        ? task.title
        : form.editField === "notes"
          ? task.notes || ""
          : "";
    if (form.editField.startsWith("step:")) {
      pageSteps.selected = form.editField.slice("step:".length);
      pageSteps.marked = null;
    }
  }

  function handlePreviewPageKey(e) {
    if (!projectsPreviewPage()) return false;
    const task = previewTask();
    const bare = !e.ctrlKey && !e.metaKey && !e.altKey;
    if (preview.editField) {
      if (e.key === "Tab") {
        e.preventDefault();
        movePageFocus(e.shiftKey, true);
        render();
      } else if (e.key === "Escape") {
        e.preventDefault();
        preview.editField = null;
        preview.editDraft = "";
        render();
      } else if (e.key === "Enter" && preview.editField === "title" && bare) {
        e.preventDefault();
        commitPreviewEdit();
        render();
      } else if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
        e.preventDefault();
        commitPreviewEdit();
        render();
      }
      return true;
    }
    if (!task) return true;
    if (["s", "n", "o", "d", "b", "r", "x", "f"].includes(e.key)) {
      clearMarks(preview);
    }
    if (previewSteps.editor) {
      if (e.key === "Escape") {
        e.preventDefault();
        previewSteps.cancel();
        render();
      } else if (e.key === "Enter" && bare) {
        e.preventDefault();
        const persists = !previewSteps.editor.id || e.shiftKey;
        if (previewSteps.save(task, e.shiftKey) && persists)
          task.updatedAt = clock();
        render();
      }
      return true;
    }
    if (previewSteps.dirty && e.key === "Escape") {
      e.preventDefault();
      previewSteps.cancel();
      render();
      return true;
    }
    if (previewSteps.dirty && e.key === "Enter" && e.shiftKey && bare) {
      e.preventDefault();
      previewSteps.save(task, true);
      task.updatedAt = clock();
      render();
      return true;
    }
    if (bare && ["Tab", "ArrowDown", "ArrowUp", "j", "k"].includes(e.key)) {
      e.preventDefault();
      previewSteps.move(
        task,
        e.shiftKey || ["ArrowUp", "k"].includes(e.key) ? -1 : 1,
      );
      render();
      root
        .querySelector(`[data-preview-step="${previewSteps.selected}"]`)
        ?.scrollIntoView({ block: "nearest" });
      return true;
    }
    if (bare && e.key === "Enter" && previewSteps.selected) {
      e.preventDefault();
      if (previewSteps.selected === "add") previewSteps.begin(task);
      else {
        previewSteps.toggle(task);
        task.updatedAt = clock();
      }
      render();
      return true;
    }
    if (bare && e.key === "a") {
      e.preventDefault();
      previewSteps.begin(task);
      render();
      return true;
    }
    if (bare && e.key === "e") {
      e.preventDefault();
      clearMarks(preview);
      if (previewSteps.selected && previewSteps.selected !== "add")
        previewSteps.begin(task, previewSteps.selected);
      else openPreviewTaskPage("title");
      render();
      return true;
    }
    if (
      bare &&
      e.key === "x" &&
      previewSteps.selected &&
      previewSteps.selected !== "add"
    ) {
      e.preventDefault();
      if (previewSteps.remove(task)) task.updatedAt = clock();
      render();
      return true;
    }
    if (bare && ["ArrowLeft", "h"].includes(e.key)) {
      e.preventDefault();
      // Like the native parked task form, leaving the page keeps an unfinished preview
      // draft available when the right seat is focused again.
      preview.page = false;
      render();
      return true;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      leavePreviewTaskPage();
      render();
      return true;
    }
    if (bare && e.key === "d") {
      e.preventDefault();
      previewSetStatus("done");
      render();
      return true;
    }
    if (bare && e.key === "b") {
      e.preventDefault();
      previewToggleBlock();
      render();
      return true;
    }
    if (bare && e.key === "r") {
      e.preventDefault();
      previewToggleReview();
      render();
      return true;
    }
    if (bare && e.key === "n") {
      e.preventDefault();
      openPreviewTaskPage("notes");
      render();
      return true;
    }
    if (bare && e.key === "?") {
      e.preventDefault();
      state.overlay = "help";
      state.helpQ = "";
      render();
      return true;
    }
    return true;
  }

  function handlePreviewBoardKey(e) {
    if (!projectsPreviewFocused() || preview.page || state.overlay)
      return false;
    const bare = !e.ctrlKey && !e.metaKey && !e.altKey;
    if (e.key === "Escape") {
      e.preventDefault();
      if (preview.searchPinned) {
        preview.searchQuery = "";
        preview.searchPinned = false;
        ensurePreviewSelection();
      } else if (preview.peekId) preview.peekId = null;
      else state.stage = "split";
      render();
      return true;
    }
    if (bare && e.shiftKey && e.key === "M") {
      e.preventDefault();
      toggleMarkMode(preview);
      preview.message = "";
      render();
      return true;
    }
    if (bare && (e.key === "j" || e.key === "ArrowDown")) {
      e.preventDefault();
      if (e.shiftKey && !preview.markMode) return true;
      if (e.shiftKey) markCurrent(preview);
      movePreview(1);
      render();
      return true;
    }
    if (bare && (e.key === "k" || e.key === "ArrowUp")) {
      e.preventDefault();
      if (e.shiftKey && !preview.markMode) return true;
      if (e.shiftKey) markCurrent(preview);
      movePreview(-1);
      render();
      return true;
    }
    if (bare && ["ArrowRight", "l"].includes(e.key)) {
      e.preventDefault();
      preview.peekId = preview.selectedId;
      render();
      return true;
    }
    if (bare && ["ArrowLeft", "h"].includes(e.key)) {
      e.preventDefault();
      if (preview.peekId) preview.peekId = null;
      else state.stage = "split";
      render();
      return true;
    }
    if (bare && e.key === " ") {
      e.preventDefault();
      if (preview.markMode) toggleMark(preview);
      render();
      return true;
    }
    if (bare && e.key === "Enter") {
      e.preventDefault();
      if (previewTask()) openPreviewTaskPage();
      render();
      return true;
    }
    if (bare && e.key === "+") {
      e.preventDefault();
      openQuickAdd("preview");
      render();
      return true;
    }
    if (bare && e.key === "s") {
      e.preventDefault();
      previewPrimaryVerb();
      render();
      return true;
    }
    if (bare && e.key === "d") {
      e.preventDefault();
      previewSetStatus("done");
      render();
      return true;
    }
    if (bare && e.key === "b") {
      e.preventDefault();
      previewToggleBlock();
      render();
      return true;
    }
    if (bare && e.key === "r") {
      e.preventDefault();
      previewToggleReview();
      render();
      return true;
    }
    if (bare && e.key === "o") {
      e.preventDefault();
      previewSetStatus("open");
      render();
      return true;
    }
    if (bare && e.key === "f") {
      e.preventDefault();
      previewFileSelected();
      render();
      return true;
    }
    if (bare && e.key === "x") {
      e.preventDefault();
      previewDeleteSelected();
      render();
      return true;
    }
    if (bare && e.key === "u") {
      e.preventDefault();
      previewUndoDelete();
      render();
      return true;
    }
    if (bare && e.key === "z") {
      e.preventDefault();
      clearMarks(preview);
      preview.drawer = !preview.drawer;
      ensurePreviewSelection();
      render();
      return true;
    }
    if (bare && e.key === "g") {
      e.preventDefault();
      clearMarks(preview);
      preview.archivedOpen = !preview.archivedOpen;
      render();
      return true;
    }
    if (bare && e.key === "e") {
      e.preventDefault();
      if (previewTask()) openPreviewTaskPage("title");
      render();
      return true;
    }
    if (bare && e.key === "n") {
      e.preventDefault();
      previewSetStatus("ready");
      render();
      return true;
    }
    if (bare && e.key === "t") {
      e.preventDefault();
      openPreviewFilter();
      render();
      return true;
    }
    if (bare && e.key === "?") {
      e.preventDefault();
      state.overlay = "help";
      state.helpQ = "";
      render();
      return true;
    }
    return false;
  }

  function onKey(e) {
    // Never swallow keys pressed on the landing chrome inside the pane
    // (pane bar, layout toggle, divider): those keep their own keyboard
    // behavior. Buttons inside the board canvas (rows, tabs, chips, verbs,
    // close boxes) also keep native activation, except while the quick-add
    // overlay is open and borrowing the frame's keys for its input.
    const el = e.target;
    if (el !== frame) {
      if (el.closest(".pane-bar, .layout-toggle, [data-divider]")) return;
      if (state.overlay !== "quick" && el.closest("button")) return;
    }
    if (e.key.toLowerCase() !== "x") {
      state.pendingDelete = null;
      preview.pendingDelete = null;
    }
    const markOwner =
      projectsPreviewFocused() || projectsPreviewPage() ? preview : state;
    const textEntryOwnsCapitalM =
      [
        "quick",
        "help",
        "palette",
        "search",
        "filter",
        "preview-filter",
      ].includes(state.overlay) ||
      Boolean(
        state.editField ||
        preview.editField ||
        steps.editor ||
        previewSteps.editor,
      );
    if (
      !textEntryOwnsCapitalM &&
      e.key === "M" &&
      e.shiftKey &&
      !e.ctrlKey &&
      !e.altKey &&
      !e.metaKey &&
      markOwner.markMode
    ) {
      e.preventDefault();
      clearMarks(markOwner);
      markOwner.message = "";
      render();
      return;
    }
    if (
      e.key === "Escape" &&
      !e.shiftKey &&
      !e.ctrlKey &&
      !e.altKey &&
      !e.metaKey &&
      markOwner.markMode
    ) {
      e.preventDefault();
      clearMarks(markOwner);
      markOwner.message = "";
      render();
      return;
    }
    if (el.id === "tsk-project-search") {
      const owner = searchOwner();
      if (e.key === "Escape") {
        e.preventDefault();
        updateSearchQuery(owner, "");
        owner.searchPinned = false;
        state.overlay = null;
        render();
        frame.focus({ preventScroll: true });
        return;
      }
      if (e.key === "Enter" && !e.ctrlKey && !e.altKey && !e.metaKey) {
        e.preventDefault();
        pinSearch(owner);
        state.overlay = null;
        render();
        frame.focus({ preventScroll: true });
        return;
      }
      return;
    }
    const wide = isWideSplit();
    const taskPageActive = taskFocus();
    if (state.overlay === "quick") {
      if (state.quickExpanded) {
        if (e.key === "Escape") {
          e.preventDefault();
          collapseQuickAdd();
          render();
          return;
        }
        if (e.key === "Tab") {
          e.preventDefault();
          movePageFocus(e.shiftKey, state.quickOwner === "preview");
          render();
          return;
        }
        if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
          e.preventDefault();
          saveExpandedDraft();
          render();
          return;
        }
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        state.overlay = null;
        state.quickOwner = "outer";
        state.refuse = "";
        frame.focus();
        render();
        return;
      }
      if (e.key === "Enter" && !e.ctrlKey && !e.altKey && !e.metaKey) {
        e.preventDefault();
        saveDraft(e.shiftKey);
        if (!e.shiftKey) frame.focus();
        render();
        return;
      }
      if (e.key === "Tab") {
        e.preventDefault();
        expandQuickAdd();
        render();
      }
      return;
    }

    if (handlePreviewPageKey(e) || handlePreviewBoardKey(e)) return;

    if (taskPageActive && !state.overlay && !state.editField) {
      const task = selectedTask();
      const bare = !e.ctrlKey && !e.metaKey && !e.altKey;
      if (steps.editor) {
        if (e.key === "Escape") {
          e.preventDefault();
          steps.cancel();
          render();
        } else if (e.key === "Enter" && bare) {
          e.preventDefault();
          const persists = !steps.editor.id || e.shiftKey;
          if (steps.save(task, e.shiftKey) && persists)
            task.updatedAt = clock();
          render();
        }
        return;
      }
      if (steps.dirty && e.key === "Escape") {
        e.preventDefault();
        steps.cancel();
        render();
        return;
      }
      if (steps.dirty && e.key === "Enter" && e.shiftKey && bare) {
        e.preventDefault();
        steps.save(task, true);
        task.updatedAt = clock();
        render();
        return;
      }
      if (bare && ["Tab", "ArrowDown", "ArrowUp", "j", "k"].includes(e.key)) {
        e.preventDefault();
        steps.move(
          task,
          e.shiftKey || ["ArrowUp", "k"].includes(e.key) ? -1 : 1,
        );
        render();
        root
          .querySelector(`[data-step="${steps.selected}"]`)
          ?.scrollIntoView({ block: "nearest" });
        return;
      }
      if (bare && e.key === "Enter" && steps.selected) {
        e.preventDefault();
        if (steps.selected === "add") steps.begin(task);
        else {
          steps.toggle(task);
          task.updatedAt = clock();
        }
        render();
        return;
      }
      if (bare && e.key === "a") {
        e.preventDefault();
        steps.begin(task);
        render();
        return;
      }
      if (bare && e.key === "e" && steps.selected && steps.selected !== "add") {
        e.preventDefault();
        steps.begin(task, steps.selected);
        render();
        return;
      }
      if (bare && e.key === "x" && steps.selected && steps.selected !== "add") {
        e.preventDefault();
        if (steps.remove(task)) task.updatedAt = clock();
        render();
        return;
      }
      steps.marked = null;
      if (steps.dirty && ["e", "n", "f", "p"].includes(e.key)) {
        e.preventDefault();
        return;
      }
    }
    if (taskPageActive && state.editField) {
      if (e.key === "Tab") {
        e.preventDefault();
        movePageFocus(e.shiftKey);
        render();
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        state.editField = null;
        frame.focus();
        render();
        return;
      }
      if (e.key === "Enter" && state.editField === "title") {
        e.preventDefault();
        commitEdit();
        render();
      }
      if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
        e.preventDefault();
        commitEdit();
        render();
      }
      return;
    }

    if (state.overlay === "help") {
      if (e.key === "Escape") {
        e.preventDefault();
        if (state.helpQ) state.helpQ = "";
        else state.overlay = null;
        render();
        return;
      }
      if (e.key === "Backspace") {
        e.preventDefault();
        state.helpQ = state.helpQ.slice(0, -1);
        render();
        return;
      }
      if (["ArrowUp", "ArrowDown", "PageUp", "PageDown"].includes(e.key)) {
        e.preventDefault();
        const body = frame.querySelector(".tsk-help-body");
        const direction = e.key === "ArrowUp" || e.key === "PageUp" ? -1 : 1;
        body?.scrollBy({
          top: direction * (e.key.startsWith("Page") ? body.clientHeight : 24),
        });
        return;
      }
      if (e.key.length === 1 && !e.altKey && !e.ctrlKey && !e.metaKey) {
        e.preventDefault();
        state.helpQ += e.key;
        render();
      }
      return;
    }

    if (state.overlay === "palette") {
      if (e.key === "Escape") {
        e.preventDefault();
        state.overlay = null;
        render();
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        const cmds = paletteCommands();
        const cmd = cmds[state.paletteI];
        state.overlay = null;
        if (cmd) cmd.run();
        render();
        return;
      }
      if (e.key === "ArrowDown" || e.key === "j") {
        e.preventDefault();
        state.paletteI += 1;
        render();
        return;
      }
      if (e.key === "ArrowUp" || e.key === "k") {
        e.preventDefault();
        state.paletteI = Math.max(0, state.paletteI - 1);
        render();
        return;
      }
      if (e.key === "Backspace") {
        e.preventDefault();
        state.paletteQ = state.paletteQ.slice(0, -1);
        state.paletteI = 0;
        render();
        return;
      }
      if (e.key.length === 1 && !e.altKey && !e.ctrlKey && !e.metaKey) {
        e.preventDefault();
        state.paletteQ += e.key;
        state.paletteI = 0;
        render();
      }
      return;
    }

    if (state.overlay === "search") {
      const owner = searchOwner();
      if (e.key === "Escape") {
        e.preventDefault();
        updateSearchQuery(owner, "");
        owner.searchPinned = false;
        state.overlay = null;
        render();
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        pinSearch(owner);
        state.overlay = null;
        render();
        return;
      }
      if (e.key === "Backspace") {
        e.preventDefault();
        updateSearchQuery(owner, owner.searchQuery.slice(0, -1));
        render();
        return;
      }
      if (e.key.length === 1 && !e.altKey && !e.ctrlKey && !e.metaKey) {
        e.preventDefault();
        updateSearchQuery(owner, owner.searchQuery + e.key);
        render();
      }
      return;
    }

    if (state.overlay === "preview-filter") {
      e.preventDefault();
      if (e.key === "Escape") state.overlay = null;
      else if (e.key === "Enter") choosePreviewFilter(state.filterI);
      else if (["ArrowDown", "j"].includes(e.key))
        state.filterI = Math.min(
          previewFilterOptions().length - 1,
          state.filterI + 1,
        );
      else if (["ArrowUp", "k"].includes(e.key))
        state.filterI = Math.max(0, state.filterI - 1);
      render();
      return;
    }
    if (state.overlay === "filter") {
      e.preventDefault();
      if (e.key === "Escape") state.overlay = null;
      else if (e.key === "Enter") chooseFilter(state.filterI);
      else if (["ArrowDown", "j"].includes(e.key))
        state.filterI = Math.min(filterOptions().length - 1, state.filterI + 1);
      else if (["ArrowUp", "k"].includes(e.key))
        state.filterI = Math.max(0, state.filterI - 1);
      render();
      return;
    }
    if (
      !state.overlay &&
      !taskPageActive &&
      ((e.key === "t" && state.focusProject) ||
        (e.key === "v" && state.tab === "projects"))
    ) {
      e.preventDefault();
      openFilter();
      render();
      return;
    }
    if (state.overlay === "picker") {
      const opts = pickerOptions();
      if (e.key === "Escape") {
        e.preventDefault();
        state.overlay = null;
        render();
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        const name = opts[state.pickerI];
        if (name === "desk") goTab("desk");
        else openProject(name);
        state.overlay = null;
        render();
        return;
      }
      if (e.key === "ArrowDown" || e.key === "j") {
        e.preventDefault();
        state.pickerI = Math.min(opts.length - 1, state.pickerI + 1);
        render();
        return;
      }
      if (e.key === "ArrowUp" || e.key === "k") {
        e.preventDefault();
        state.pickerI = Math.max(0, state.pickerI - 1);
        render();
      }
      return;
    }

    const alt = e.altKey || e.ctrlKey || e.metaKey;
    if (
      taskPageActive &&
      !e.altKey &&
      !e.ctrlKey &&
      [
        "j",
        "k",
        "ArrowDown",
        "ArrowUp",
        "1",
        "2",
        "3",
        "+",
        "z",
        "D",
        ":",
      ].includes(e.key)
    ) {
      e.preventDefault();
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      if (state.searchPinned) {
        state.searchQuery = "";
        state.searchPinned = false;
        ensureSelection(buildRows());
      } else if (taskPageActive) {
        state.overlay = null;
        leaveTaskPage();
      } else if (state.peekId) state.peekId = null;
      else if (wide && state.stage === "split") {
        if (projectsOverview() && previewHasUnsavedWork())
          preview.message = "save or cancel edits before switching tasks";
        else stageLeft();
      }
      // The terminal exits at the full-board root on any tab. A browser demo has no
      // process to exit, so release keyboard focus without changing the selected tab.
      else frame.blur();
      render();
      return;
    }
    if (e.key === "/" && !taskPageActive) {
      e.preventDefault();
      searchOwner().searchPinned = false;
      state.overlay = "search";
      render();
      return;
    }
    if (e.key === "?") {
      e.preventDefault();
      state.overlay = "help";
      state.helpQ = "";
      render();
      return;
    }
    if (e.key === ":") {
      e.preventDefault();
      state.overlay = "palette";
      state.paletteQ = "";
      state.paletteI = 0;
      render();
      return;
    }
    if (e.key === "+") {
      e.preventDefault();
      openQuickAdd();
      render();
      return;
    }
    if (e.key === "1" || e.key === "2" || e.key === "3") {
      e.preventDefault();
      goTab(TABS[Number(e.key) - 1][0]);
      render();
      return;
    }
    if (e.key === "p" || e.key === "P") {
      e.preventDefault();
      state.overlay = "picker";
      state.pickerI = Math.max(
        0,
        pickerOptions().indexOf(state.selectedProject),
      );
      render();
      return;
    }
    if (e.key === "z" || e.key === "D") {
      e.preventDefault();
      clearMarks();
      state.drawer = !state.drawer;
      render();
      return;
    }
    if (e.key === "g" && !e.altKey && !e.ctrlKey && !e.metaKey) {
      e.preventDefault();
      toggleAllGroups();
      render();
      return;
    }
    if (!taskPageActive && !alt && e.shiftKey && e.key === "M") {
      e.preventDefault();
      toggleMarkMode();
      state.message = "";
      render();
      return;
    }
    if (e.key === "j" || e.key === "ArrowDown") {
      e.preventDefault();
      if (e.shiftKey && !state.markMode) return;
      if (e.shiftKey) markCurrent();
      move(1);
      render();
      return;
    }
    if (e.key === "k" || e.key === "ArrowUp") {
      e.preventDefault();
      if (e.shiftKey && !state.markMode) return;
      if (e.shiftKey) markCurrent();
      move(-1);
      render();
      return;
    }
    if (!alt && ["ArrowRight", "l"].includes(e.key)) {
      e.preventDefault();
      if (wide) {
        stageRight();
        state.peekId = null;
      } else if (!taskPageActive) {
        state.peekId = state.selectedId;
      }
      render();
      return;
    }
    if (!alt && ["ArrowLeft", "h"].includes(e.key)) {
      e.preventDefault();
      if (wide) stageLeft();
      else state.peekId = null;
      render();
      return;
    }
    if (!taskPageActive && e.key === " ") {
      e.preventDefault();
      if (state.markMode) toggleMark();
      render();
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      if (taskPageActive && state.stage === "page") leaveTaskPage();
      else {
        const row = selectedRow();
        if (row?.kind === "project") openProject(row.project);
        else if (row?.kind === "inbox") {
          clearMarks();
          state.inboxOpen = !state.inboxOpen;
        } else if (row?.kind === "archived") {
          clearMarks();
          state.archivedOpen = !state.archivedOpen;
        } else openFullPage();
      }
      render();
      return;
    }
    if (
      taskPageActive &&
      ["s", "n", "o", "d", "b", "r", "x", "f"].includes(e.key)
    ) {
      clearMarks();
    }
    if (
      !state.markedIds.size &&
      ["s", "n", "o", "d", "b", "r", "x", "f"].includes(e.key) &&
      selectedRow()?.kind !== "task"
    ) {
      clearMarks();
      return;
    }
    if (e.key === "s") {
      e.preventDefault();
      primaryVerb();
      render();
      return;
    }
    if (e.key === "d") {
      e.preventDefault();
      setStatus("done");
      render();
      return;
    }
    if (e.key === "o") {
      e.preventDefault();
      setStatus("open");
      render();
      return;
    }
    if (e.key === "b") {
      e.preventDefault();
      toggleBlock();
      render();
      return;
    }
    if (e.key === "r") {
      e.preventDefault();
      toggleReview();
      render();
      return;
    }
    if (e.key === "f") {
      e.preventDefault();
      fileSelected();
      render();
      return;
    }
    if (e.key === "x") {
      e.preventDefault();
      deleteSelected();
      render();
      return;
    }
    if (e.key === "u") {
      e.preventDefault();
      undoDelete();
      render();
      return;
    }
    if (e.key === "e") {
      e.preventDefault();
      clearMarks();
      state.pendingDelete = null;
      const task = selectedTask();
      if (task) {
        enterTaskStage();
        state.editField = "title";
        state.editDraft = task.title;
      }
      render();
      return;
    }
    if (e.key === "n") {
      e.preventDefault();
      setStatus("ready");
      render();
      return;
    }
    if (e.key === "q" && alt) {
      e.preventDefault();
      frame.blur();
      render();
    }
  }

  let lastClick = { id: null, at: 0 };
  const cancelReflowClick = () => {
    delete lastClick.reflow;
  };
  frame.addEventListener(
    "keydown",
    () => {
      cancelReflowClick();
      lastClick = { id: null, at: 0 };
    },
    true,
  );
  root.addEventListener("wheel", cancelReflowClick, { passive: true });
  root.addEventListener("pointermove", (e) => {
    if (e.buttons) cancelReflowClick();
  });
  root.addEventListener("pointerdown", (e) => {
    const previous = lastClick.reflow;
    if (
      e.button !== 0 ||
      (previous && (e.clientX !== previous.x || e.clientY !== previous.y))
    )
      cancelReflowClick();
  });
  root.addEventListener("pointercancel", cancelReflowClick);
  window.addEventListener("resize", cancelReflowClick);
  window.addEventListener("scroll", cancelReflowClick, true);

  root.addEventListener("click", (e) => {
    const deleteControl = e.target.closest(
      '[data-verb="delete"], [data-page-verb="delete"], [data-preview-page-verb="delete"]',
    );
    if (!deleteControl) {
      state.pendingDelete = null;
      preview.pendingDelete = null;
    }
    const previous = lastClick.reflow;
    cancelReflowClick();
    // Reflow must not turn the second click into a newly exposed task control.
    if (
      previous &&
      e.detail > 0 &&
      state.stage === "split" &&
      state.selectedId === lastClick.id &&
      Date.now() - lastClick.at < 350 &&
      e.clientX === previous.x &&
      e.clientY === previous.y &&
      frame.clientWidth === previous.width &&
      !state.editField &&
      !steps.editor &&
      !steps.dirty
    ) {
      lastClick = { id: null, at: 0 };
      openFullPage();
      render();
      return;
    }
    // A stage A click inside the task column slides to G first, then the control runs.
    const previewFilterControl = e.target.closest("[data-preview-filter]");
    if (previewFilterControl && projectsPreviewActive()) {
      if (state.stage === "split") stageRight();
      openPreviewFilter();
      render();
      return;
    }
    const previewFilterOption = e.target.closest(
      "[data-preview-filter-option]",
    );
    if (previewFilterOption && state.overlay === "preview-filter") {
      choosePreviewFilter(
        Number(previewFilterOption.dataset.previewFilterOption),
      );
      render();
      return;
    }
    const filterControl = e.target.closest("[data-filter]");
    if (filterControl) {
      openFilter();
      render();
      return;
    }
    const filterOption = e.target.closest("[data-filter-option]");
    if (filterOption) {
      chooseFilter(Number(filterOption.dataset.filterOption));
      render();
      return;
    }
    const previewStepTarget = e.target.closest(
      "[data-preview-step], [data-preview-step-add]",
    );
    if (
      projectsPreviewPage() &&
      previewStepTarget &&
      e.target.id !== "tsk-preview-step-edit"
    ) {
      const task = previewTask();
      if (
        previewSteps.editor &&
        !previewSteps.editor.text.trim() &&
        !previewSteps.editor.id
      )
        previewSteps.cancel();
      else if (previewSteps.editor && !previewSteps.save(task, false)) {
        render();
        return;
      }
      if (previewStepTarget.hasAttribute("data-preview-step-add"))
        previewSteps.begin(task);
      else previewSteps.selected = previewStepTarget.dataset.previewStep;
      render();
      return;
    }
    const stepTarget = e.target.closest("[data-step], [data-step-add]");
    if (stepTarget && e.target.id !== "tsk-step-edit") {
      const task = selectedTask();
      if (steps.editor && !steps.editor.text.trim() && !steps.editor.id)
        steps.cancel();
      else if (steps.editor && !steps.save(task, false)) {
        render();
        return;
      }
      enterTaskStage();
      if (stepTarget.hasAttribute("data-step-add")) steps.begin(task);
      else {
        steps.selected = stepTarget.dataset.step;
        steps.marked = null;
      }
      render();
      return;
    }
    const previewArchivedHeader = e.target.closest("[data-preview-archived]");
    if (previewArchivedHeader && projectsPreviewActive()) {
      clearMarks(preview);
      preview.archivedOpen = !preview.archivedOpen;
      ensurePreviewSelection();
      render();
      return;
    }
    const previewTaskRow = e.target.closest("[data-preview-task]");
    if (previewTaskRow && projectsPreviewActive()) {
      const copyTarget = e.target.closest("[data-copy-task]");
      if (
        copyTarget &&
        (!preview.markMode || e.ctrlKey || e.metaKey || e.altKey || e.shiftKey)
      ) {
        const task = state.tasks.find(
          (item) => item.id === copyTarget.getAttribute("data-copy-task"),
        );
        if (task) copyTaskIdentifier(task);
        render();
        return;
      }
      const id = previewTaskRow.getAttribute("data-preview-task");
      if (previewHasUnsavedWork() && id !== preview.selectedId) return;
      const now = Date.now();
      if (state.stage === "split") stageRight();
      const markClick =
        preview.markMode &&
        !e.ctrlKey &&
        !e.metaKey &&
        !e.altKey &&
        !e.shiftKey;
      if (markClick) {
        preview.selectedId = id;
        toggleMark(preview);
        preview.peekId = null;
        frame.focus({ preventScroll: true });
      } else if (
        preview.selectedId === id &&
        now - (preview.lastClick || 0) < 350
      ) {
        openPreviewTaskPage();
      } else {
        preview.selectedId = id;
        preview.peekId = null;
      }
      preview.lastClick = markClick ? 0 : now;
      render();
      return;
    }
    const taskColumn = e.target.closest(
      ".tsk-task-column, .tsk-project-preview",
    );
    if (taskColumn && isWideSplit() && state.stage === "split") {
      stageRight();
      render();
      return;
    }
    const close = e.target.closest("[data-close]");
    if (close) {
      state.overlay = null;
      state.quickOwner = "outer";
      state.paletteQ = "";
      state.helpQ = "";
      frame.focus();
      render();
      return;
    }
    const copy = e.target.closest("[data-copy-task]");
    if (
      copy &&
      (!state.markMode || e.ctrlKey || e.metaKey || e.altKey || e.shiftKey)
    ) {
      const task = state.tasks.find(
        (item) => item.id === copy.getAttribute("data-copy-task"),
      );
      if (task) copyTaskIdentifier(task);
      render();
      return;
    }
    const tab = e.target.closest("[data-tab]");
    if (tab) {
      goTab(tab.getAttribute("data-tab"));
      render();
      return;
    }
    const chip = e.target.closest("[data-chip]");
    if (chip) {
      state.overlay = "picker";
      state.pickerI = Math.max(
        0,
        pickerOptions().indexOf(state.selectedProject),
      );
      render();
      return;
    }
    const inboxHeader = e.target.closest("[data-inbox-header]");
    if (inboxHeader) {
      clearMarks();
      state.inboxOpen = !state.inboxOpen;
      render();
      return;
    }
    const archivedHeader = e.target.closest("[data-archived-header]");
    if (archivedHeader) {
      clearMarks();
      state.archivedOpen = !state.archivedOpen;
      render();
      return;
    }
    const projectRow = e.target.closest("[data-project-row]");
    if (projectRow) {
      const id = projectRow.dataset.navId;
      const project = projectRow.dataset.projectRow;
      const now = Date.now();
      if (lastClick.id === id && now - lastClick.at < 350) {
        if (
          projectsOverview() &&
          projectsPreviewActive() &&
          previewHasUnsavedWork()
        ) {
          preview.message = "save or cancel edits before switching tasks";
          render();
          return;
        }
        openProject(project);
      } else {
        if (
          projectsOverview() &&
          projectsPreviewActive() &&
          preview.project !== project &&
          !bindProjectPreviewFor(project)
        ) {
          render();
          return;
        }
        state.selectedId = id;
        if (projectsOverview() && isWideSplit()) {
          if (state.stage === "board") openProjectPreview();
          else if (state.stage === "rail") {
            bindProjectPreview();
            state.stage = "split";
          } else bindProjectPreview();
        }
      }
      lastClick = { id, at: now };
      render();
      return;
    }
    const group = e.target.closest("[data-collapse]");
    if (group) {
      clearMarks();
      const now = Date.now();
      const key = group.getAttribute("data-collapse");
      const project = group.getAttribute("data-project");
      if (lastClick.id === key && now - lastClick.at < 350 && project) {
        if (project === "desk") goTab("desk");
        else openProject(project);
      } else if (state.collapsed.has(key)) state.collapsed.delete(key);
      else state.collapsed.add(key);
      lastClick = { id: key, at: now };
      render();
      return;
    }
    const row = e.target.closest("[data-task]");
    if (row) {
      const id = row.getAttribute("data-task");
      if ((steps.dirty || steps.editor) && id !== state.selectedId) return;
      if (
        state.markMode &&
        !e.ctrlKey &&
        !e.metaKey &&
        !e.altKey &&
        !e.shiftKey
      ) {
        state.selectedId = id;
        toggleMark();
        state.peekId = null;
        lastClick = { id: null, at: 0 };
        frame.focus({ preventScroll: true });
        render();
        return;
      }
      const now = Date.now();
      const reflow =
        e.detail > 0 &&
        isWideSplit() &&
        (state.stage === "board" || state.stage === "rail") &&
        !state.editField &&
        !steps.editor &&
        !steps.dirty
          ? { x: e.clientX, y: e.clientY, width: frame.clientWidth }
          : null;
      if (lastClick.id === id && now - lastClick.at < 350) {
        state.selectedId = id;
        state.peekId = null;
        openFullPage();
      } else if (isWideSplit()) {
        if (state.stage === "board" || state.stage === "rail")
          state.stage = "split";
        state.selectedId = id;
        state.peekId = null;
        state.overlay = null;
      } else {
        state.selectedId = id;
        state.peekId = state.peekId === id ? null : id;
      }
      lastClick = { id, at: now, reflow };
      render();
      return;
    }
    const previewDrawer = e.target.closest("[data-preview-drawer]");
    if (previewDrawer) {
      clearMarks(preview);
      preview.drawer = !preview.drawer;
      ensurePreviewSelection();
      render();
      return;
    }
    const previewPageVerb = e.target.closest("[data-preview-page-verb]");
    if (previewPageVerb) {
      runPreviewPageVerb(
        previewPageVerb.getAttribute("data-preview-page-verb"),
      );
      render();
      return;
    }
    const previewVerb = e.target.closest("[data-preview-verb]");
    if (previewVerb) {
      runPreviewVerb(previewVerb.getAttribute("data-preview-verb"));
      render();
      return;
    }
    const drawer = e.target.closest("[data-drawer]");
    if (drawer) {
      clearMarks();
      state.drawer = !state.drawer;
      render();
      return;
    }
    const cmd = e.target.closest("[data-cmd]");
    if (cmd && state.overlay === "palette") {
      const hit = paletteCommands().find(
        (c) => c.id === cmd.getAttribute("data-cmd"),
      );
      state.overlay = null;
      if (hit) hit.run();
      render();
      return;
    }
    const hint = e.target.closest("[data-hint]");
    if (hint && runHint(hint.getAttribute("data-hint"))) {
      render();
      return;
    }
    const pageVerb = e.target.closest("[data-page-verb]");
    if (pageVerb) {
      runPageVerb(pageVerb.getAttribute("data-page-verb"));
      render();
      return;
    }
    const verb = e.target.closest("[data-verb]");
    if (verb) {
      runVerb(verb.getAttribute("data-verb"));
      render();
      return;
    }
    const pick = e.target.closest("[data-pick]");
    if (pick) {
      const name = pick.getAttribute("data-pick");
      if (name === "desk") goTab("desk");
      else openProject(name);
      state.overlay = null;
      render();
    }
  });

  frame.addEventListener("keydown", onKey);
  // The landing page's layout toggle asks for a stage directly (full terminal opens in split).
  frame.addEventListener("tsk:set-stage", (e) => {
    const stage = e.detail;
    if (!["board", "split", "rail", "page"].includes(stage)) return;
    if (projectsOverview()) {
      if (stage === "page" || (stage === "board" && previewHasUnsavedWork()))
        return;
      if (stage === "board") dropProjectPreview();
      else {
        if (!bindProjectPreview()) return;
        leavePreviewTaskPage();
      }
      state.stage = stage;
      state.stageOrigin = null;
      state.peekId = null;
      if (state.overlay !== "quick") state.overlay = null;
      render();
      return;
    }
    if (stage !== "board" && !state.selectedId) return;
    state.stage = stage;
    state.stageOrigin = null;
    state.peekId = null;
    if (state.overlay !== "quick") state.overlay = null;
    render();
  });
  new ResizeObserver(() => render()).observe(root);
  frame.addEventListener("focusin", () => frame.classList.add("is-focused"));
  frame.addEventListener("focusout", (e) => {
    if (!frame.contains(e.relatedTarget)) frame.classList.remove("is-focused");
  });

  render();
  frame.dispatchEvent(new CustomEvent("tsk:ready"));
})();
