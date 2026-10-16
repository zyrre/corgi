function emptySnapshot() {
  return { agents: [] }
}

function _object(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value)
}

function parseSnapshot(raw) {
  var text = String(raw || "")
  // A bar widget should not try to parse an unexpectedly large command result.
  if (text.length === 0 || text.length > 1024 * 1024) return null

  try {
    var payload = JSON.parse(text)
    if (!_object(payload)) return null

    // `herdr api snapshot` currently returns a normal API envelope. Keep the
    // direct forms too, so this continues to work if the CLI unwraps it later.
    var result = _object(payload.result) ? payload.result : payload
    if (_object(result.snapshot)) return result.snapshot
    if (_object(payload.snapshot)) return payload.snapshot
    if (Array.isArray(result.agents)) return result
    return null
  } catch (error) {
    return null
  }
}

function stateFor(agent) {
  var value = String((agent && (agent.agent_status || agent.status || agent.state)) || "").toLowerCase()
  if (value === "working" || value === "busy" || value === "running") return "working"
  if (value === "blocked" || value === "waiting" || value === "input" || value === "question") return "blocked"
  if (value === "done" || value === "completed") return "done"
  if (value === "idle" || value === "ready") return "idle"
  return "unknown"
}

function summarize(snapshot) {
  var agents = snapshot && Array.isArray(snapshot.agents) ? snapshot.agents : []
  var summary = {
    total: agents.length,
    working: 0,
    blocked: 0,
    done: 0,
    idle: 0,
    unknown: 0
  }

  for (var index = 0; index < agents.length; index++) {
    var state = stateFor(agents[index])
    summary[state] += 1
  }
  return summary
}

function stateColor(summary, accent, urgent, foreground) {
  if ((summary.blocked || 0) > 0) return urgent
  if ((summary.working || 0) > 0) return accent
  if ((summary.done || 0) > 0) return accent
  return foreground
}

function compactText(summary, connected) {
  if (!connected) return "Corgi"
  if ((summary.total || 0) === 0) return "Corgi · 0"

  var parts = []
  if ((summary.working || 0) > 0) parts.push(String(summary.working) + " work")
  if ((summary.blocked || 0) > 0) parts.push(String(summary.blocked) + " blocked")
  if ((summary.done || 0) > 0) parts.push(String(summary.done) + " done")
  if (parts.length === 0) parts.push(String(summary.total) + " idle")
  return "Corgi · " + parts.join(" · ")
}

function tooltip(summary, connected) {
  if (!connected) return "Corgi · Herdr unavailable — click to open compact dashboard"
  if ((summary.total || 0) === 0) return "Corgi · No recognized agents — click to open compact dashboard"

  var parts = []
  if ((summary.working || 0) > 0) parts.push(String(summary.working) + " working")
  if ((summary.blocked || 0) > 0) parts.push(String(summary.blocked) + " blocked")
  if ((summary.done || 0) > 0) parts.push(String(summary.done) + " done")
  if ((summary.idle || 0) > 0) parts.push(String(summary.idle) + " idle")
  if ((summary.unknown || 0) > 0) parts.push(String(summary.unknown) + " unknown")
  return "Corgi · " + parts.join(", ") + " (" + String(summary.total) + " total)"
}

function paneIdFromResponse(raw) {
  var text = String(raw || "")
  if (text.length === 0 || text.length > 256 * 1024) return ""

  try {
    var payload = JSON.parse(text)
    var result = _object(payload.result) ? payload.result : payload
    var pluginPane = _object(result.plugin_pane) ? result.plugin_pane : null
    var pane = pluginPane && _object(pluginPane.pane) ? pluginPane.pane : null
    var paneId = pane ? String(pane.pane_id || "") : ""
    // Keep malformed output from ever reaching the command argv on a later click.
    return /^[^\r\n]{1,255}$/.test(paneId) ? paneId : ""
  } catch (error) {
    return ""
  }
}

// Match the dashboard's contiguous project runs and explicit card metadata.
// Older binaries omit the flags: their agents remain individually accessible.
function projectGroups(snapshot) {
  var agents = snapshot && Array.isArray(snapshot.agents) ? snapshot.agents : []
  var groups = []
  agents.forEach(function(agent) {
    var last = groups[groups.length - 1]
    if (!last || last.project !== agent.project_group) {
      last = {project: agent.project_group, agents: [], card: agent.supervisor === true && agent.scratch !== true}
      groups.push(last)
    }
    last.agents.push(agent)
  })
  return groups
}

function workerCounts(agents) {
  var counts = {total: Math.max(0, agents.length - 1), blocked: 0, merge: 0, working: 0, done: 0, idle: 0, unknown: 0}
  agents.slice(1).forEach(function(agent) {
    counts[agent.ready_to_merge === true ? "merge" : stateFor(agent)] += 1
  })
  return counts
}

function workerSummarySegments(agents) {
  var counts = workerCounts(agents)
  var parts = [{text: counts.total === 0 ? "no workers" : counts.total + (counts.total === 1 ? " worker" : " workers"), tone: counts.total ? "foreground" : "muted"}]
  // The dashboard's urgency order and marks; merge readiness comes from its
  // state-bound marker classification, never a task name or raw done status.
  ;[["blocked", "▲", "yellow"], ["merge", "⇡", "magenta"], ["working", "●", "cyan"],
    ["done", "✓", "green"], ["idle", "○", "muted"], ["unknown", "?", "magenta"]].forEach(function(state) {
    if (counts[state[0]]) parts.push({text: state[1] + " " + counts[state[0]], label: counts[state[0]] + " " + state[0], tone: state[2]})
  })
  return parts
}

function workerSummary(agents) {
  return workerSummarySegments(agents).map(function(part) { return part.text }).join(" · ")
}

function dashboardRows(snapshot, expandedPaneId, expandedProjects) {
  var rows = [], expanded = expandedProjects || []
  projectGroups(snapshot).forEach(function(group) {
    var shown = expandedPaneId ? group.agents.filter(function(agent) { return agent.pane_id === expandedPaneId })
      : group.card && expanded.indexOf(group.project) < 0 ? group.agents.slice(0, 1) : group.agents
    if (!shown.length) return
    rows.push({heading: true, project: group.project || "Other", projectKey: group.project,
      card: group.card, projectExpanded: expanded.indexOf(group.project) >= 0,
      supervisorPaneId: group.agents[0].pane_id,
      workerSummary: group.card ? workerSummary(group.agents) : "",
      workerSegments: group.card ? workerSummarySegments(group.agents) : [],
      workerCounts: workerCounts(group.card ? group.agents : [])})
    shown.forEach(function(agent) {
      rows.push(Object.assign({}, agent, {collapsedProject: group.card && !expandedPaneId && expanded.indexOf(group.project) < 0}))
    })
  })
  return rows
}

function projectForPane(snapshot, paneId) {
  return projectGroups(snapshot).filter(function(group) {
    return group.agents.some(function(agent) { return agent.pane_id === paneId })
  })[0] || null
}

function reconcileNavigation(snapshot, expandedProjects, expandedPaneId, selectedPaneId) {
  var groups = projectGroups(snapshot)
  var projects = expandedProjects.filter(function(project) {
    return groups.some(function(group) { return group.card && group.project === project })
  })
  var pane = projectForPane(snapshot, expandedPaneId) ? expandedPaneId : ""
  var rows = dashboardRows(snapshot, pane, projects).filter(function(row) { return !row.heading })
  var selected = rows.some(function(row) { return row.pane_id === selectedPaneId }) ? selectedPaneId : (rows[0] || {}).pane_id || ""
  return {projects: projects, pane: pane, selected: selected}
}

// One activation has exactly one effect: reveal a project OR toggle a transcript.
function activateAgent(snapshot, expandedProjects, expandedPaneId, paneId) {
  var group = projectForPane(snapshot, paneId), projects = expandedProjects.slice()
  if (!group) return reconcileNavigation(snapshot, projects, expandedPaneId, "")
  if (group.card && projects.indexOf(group.project) < 0) {
    projects.push(group.project)
    return {projects: projects, pane: "", selected: paneId}
  }
  return {projects: projects, pane: expandedPaneId === paneId ? "" : paneId, selected: paneId}
}

function toggleProject(snapshot, expandedProjects, project, supervisorPaneId) {
  var projects = expandedProjects.slice(), index = projects.indexOf(project)
  if (index < 0) projects.push(project)
  else projects.splice(index, 1)
  return reconcileNavigation(snapshot, projects, "", supervisorPaneId)
}

function backNavigation(snapshot, expandedProjects, expandedPaneId, selectedPaneId) {
  if (expandedPaneId) return reconcileNavigation(snapshot, expandedProjects, "", selectedPaneId)
  var group = projectForPane(snapshot, selectedPaneId)
  var project = group && group.card && expandedProjects.indexOf(group.project) >= 0 ? group.project
    : expandedProjects.length ? expandedProjects[expandedProjects.length - 1] : null
  if (project === null) return null // The next back closes the popup.
  var target = projectGroups(snapshot).filter(function(candidate) { return candidate.project === project })[0]
  return toggleProject(snapshot, expandedProjects, project, target ? target.agents[0].pane_id : "")
}

function expandedEntries(agent, entries) {
  if (entries.length > 0) return entries
  if (!agent) return []
  return [
    {kind: agent.message_kind || "message", text: agent.message || "No message yet"},
    {kind: agent.tool_kind || "tool", text: agent.tool || "No command yet"},
    {kind: "ready", text: "This session has not written any transcript turns yet"}
  ]
}

function latestReplyIndex(entries) {
  for (var i = 0; i < entries.length; i++) {
    if (entries[i].kind === "message" || entries[i].kind === "question") return i
  }
  return 0
}

function singleLine(value) {
  return String(value || "").replace(/\s+/g, " ").trim()
}

function details(agent) {
  var parts = []
  if (agent.model) parts.push(agent.model + (agent.effort ? " / " + agent.effort : ""))
  if (agent.context_percent !== null && agent.context_percent !== undefined) parts.push(agent.context_percent + "% ctx")
  if (agent.cache_seconds !== null && agent.cache_seconds !== undefined) parts.push(agent.cache_seconds > 0 ? "cache " + (agent.cache_estimated ? "≈" : "") + Math.ceil(agent.cache_seconds / 60) + "m" : (agent.cache_estimated ? "cache possibly cold" : "cache cold"))
  if (agent.checkout) parts.push("⑂ " + agent.checkout)
  return singleLine(parts.join(" · "))
}

function agentColor(agent, accent, urgent, foreground, muted, success) {
  var state = stateFor(agent)
  return state === "blocked" ? urgent : state === "done" ? success : state === "working" ? accent : state === "idle" ? muted : foreground
}

function herdLines(summary) {
  var rows = [
    ["●", summary.working, "WORKING", "cyan"],
    ["!", summary.blocked, "BLOCKED", "yellow"],
    ["⇡", 0, "MERGE", "magenta"],
    ["✓", summary.done, "DONE", "green"],
    ["·", summary.idle + summary.unknown, "IDLE", "muted"]
  ]
  return rows.map(function(row) {
    // Compatibility for older binaries without the shared HERD data. They
    // cannot classify merge readiness; a current binary supplies these spans.
    return {spans: [
      {text: " " + row[0] + " ", cells: 3, tone: row[3], bold: true},
      {text: String(row[1]), cells: String(row[1]).length, tone: row[3], bold: true},
      {text: " " + row[2], cells: row[2].length + 1, tone: row[3], bold: true}
    ]}
  })
}

// Escape all session text before it enters the colored rich-text status line.
function escapeHtml(value) {
  return String(value || "").replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;")
}

function indexedColor(index) {
  var channels
  if (index >= 232) {
    var gray = 8 + (index - 232) * 10
    channels = [gray, gray, gray]
  } else {
    var cube = [0, 95, 135, 175, 215, 255], value = index - 16
    channels = [cube[Math.floor(value / 36)], cube[Math.floor(value / 6) % 6], cube[value % 6]]
  }
  return "#" + channels.map(function(channel) { return ("0" + channel.toString(16)).slice(-2) }).join("")
}

function detailsHtml(agent, palette) {
  var spans = agent.details || [{text: " · " + details(agent), tone: "muted"}]
  return spans.map(function(span) {
    var color = span.indexed !== null && span.indexed !== undefined ? indexedColor(span.indexed) : palette[span.tone] || palette.foreground
    var text = escapeHtml(String(span.text || "").replace(/\s+/g, " "))
    return '<span style="color:' + escapeHtml(color) + '">' + (span.bold ? '<b>' + text + '</b>' : text) + '</span>'
  }).join("")
}

function activityMarker(kind, fallback) {
  return {command: "$", tool: "●", prompt: "»", message: "›", thinking: "…", question: "?", ready: "○"}[kind] || fallback
}

function activityColor(kind, palette) {
  return palette[{command: "cyan", tool: "cyan", prompt: "green", message: "foreground", thinking: "cyan", question: "yellow", ready: "muted"}[kind] || "foreground"]
}
