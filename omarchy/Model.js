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

function dashboardRows(snapshot, expandedPaneId) {
  var agents = snapshot && Array.isArray(snapshot.agents) ? snapshot.agents : []
  var rows = [], previous = null
  for (var i = 0; i < agents.length; i++) {
    var agent = agents[i]
    if (expandedPaneId && agent.pane_id !== expandedPaneId) continue
    if (agent.project_group !== previous) {
      previous = agent.project_group
      rows.push({heading: true, project: previous || "Other"})
    }
    rows.push(agent)
  }
  return rows
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
    ["Σ", summary.total, "TOTAL", "magenta"],
    ["●", summary.working, "WORKING", "cyan"],
    ["!", summary.blocked, "BLOCKED", "yellow"],
    ["✓", summary.done, "DONE", "green"],
    ["·", summary.idle + summary.unknown, "IDLE", "muted"]
  ]
  return rows.map(function(row) {
    return {right: false, spans: [{text: " " + row[0] + " " + row[1] + " " + row[2], tone: row[3], bold: true}]}
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
