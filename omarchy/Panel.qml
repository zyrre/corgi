import QtQuick
import QtQuick.Controls
import QtQuick.Controls as Controls
import QtQuick.Layouts
import QtQuick.Effects
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui
import "Model.js" as Model

Panel {
  id: root

  moduleName: "io.github.zyrre.corgi"
  ipcTarget: moduleName
  manageIpc: false

  function openNewAgent() {
    if (newAgentDialog.busy || actionState || promptPaneId) return
    root.open()
    newAgentOpen = true
    Qt.callLater(function() { newAgentDialog.begin() })
  }

  IpcHandler {
    target: root.ipcTarget
    function open(): void { root.open() }
    function close(): void { root.close() }
    function show(): void { root.open() }
    function hide(): void { root.close() }
    function toggle(): void { root.toggle() }
    function newAgent(): void { root.openNewAgent() }
  }
  readonly property bool vertical: bar && (bar.position === "left" || bar.position === "right")
  readonly property string fontFamily: "monospace"
  // A subtle horizontal correction for the SVG beside the terminal artwork.
  readonly property real mascotWidthScale: 0.97
  property color wordmarkColor: Color.accent
  property var headerPalette: ({foreground: Color.foreground, muted: Color.muted, cyan: Color.accent, green: Color.accent, yellow: Color.foreground, red: Color.urgent, magenta: Color.accent})
  readonly property string binaryPath: String(setting("binaryPath", "corgi"))
  readonly property string socketPath: String(setting("socketPath", Quickshell.env("HOME") + "/.config/herdr/herdr.sock"))
  property string errorText: ""
  readonly property var rows: Model.dashboardRows(snapshot, expandedPaneId)
  readonly property string actionBinaryPath: decodeURIComponent(Qt.resolvedUrl("corgi-actions").toString().replace(/^file:\/\//, ""))
  property bool newAgentOpen: false
  property var actionState: null
  property string selectedPaneId: ""
  property string expandedPaneId: ""
  property string promptPaneId: ""
  property string promptLabel: ""
  property string noticeText: ""
  property var transcriptEntries: []
  readonly property var expandedAgent: (snapshot.agents || []).find(function(agent) { return agent.pane_id === root.expandedPaneId }) || null
  readonly property var expandedEntries: Model.expandedEntries(expandedAgent, transcriptEntries)
  readonly property int latestReplyIndex: Model.latestReplyIndex(expandedEntries)

  onOpenedChanged: {
    if (opened) Qt.callLater(function() { popupContent.forceActiveFocus() })
    else {
      newAgentOpen = false
      expandedPaneId = ""
      dismissAction()
      promptPaneId = ""
      promptInput.text = ""
    }
  }

  function beginPrompt(targetPaneId) {
    if (actionState || actionProcess.running || promptPaneId || promptProcess.running) return
    var paneId = targetPaneId || expandedPaneId || selectedPaneId || ((snapshot.agents || [])[0] || {}).pane_id
    var agent = (snapshot.agents || []).find(function(agent) { return agent.pane_id === paneId })
    if (!connected || !agent) { errorText = "No agent selected"; return }
    if (agent.state === "blocked") { errorText = "Blocked agents must be handled in their own pane"; return }
    errorText = ""
    noticeText = ""
    promptPaneId = agent.pane_id
    promptLabel = agent.task || agent.name || agent.pane_id
    promptInput.text = ""
    Qt.callLater(function() { promptInput.forceActiveFocus() })
  }

  component SessionActionButton: Controls.Button {
    id: sessionButton
    required property color tone
    font.family: root.fontFamily
    font.pixelSize: Style.space(11)
    padding: Style.space(4)
    verticalPadding: 0
    hoverEnabled: true
    contentItem: Text {
      text: sessionButton.text
      color: sessionButton.tone
      font: sessionButton.font
      horizontalAlignment: Text.AlignHCenter
      verticalAlignment: Text.AlignVCenter
      opacity: sessionButton.enabled ? 1 : 0.4
    }
    background: Rectangle {
      radius: Style.space(3)
      color: sessionButton.down ? Qt.alpha(sessionButton.tone, 0.35)
        : sessionButton.hovered ? Qt.alpha(sessionButton.tone, 0.24) : Qt.alpha(sessionButton.tone, 0.12)
      border.color: sessionButton.tone
      opacity: sessionButton.enabled ? 1 : 0.3
    }
    HoverHandler { cursorShape: Qt.PointingHandCursor }
    ToolTip.visible: hovered
    ToolTip.delay: 500
    ToolTip.text: Accessible.name
  }

  function cancelPrompt() {
    if (promptProcess.running) return
    promptPaneId = ""
    promptInput.text = ""
    popupContent.forceActiveFocus()
  }

  function submitPrompt() {
    if (!promptPaneId || promptProcess.running || !promptInput.text.trim()) return
    errorText = ""
    promptProcess.command = ["env", "HERDR_SOCKET_PATH=" + socketPath, "herdr", "agent", "prompt", promptPaneId, promptInput.text.trim()]
    promptProcess.running = true
  }

  function toggleAgent(paneId) {
    selectedPaneId = paneId
    if (expandedPaneId === paneId) expandedPaneId = ""
    else {
      transcriptEntries = []
      expandedPaneId = paneId
      agentScroll.contentItem.contentY = 0
      refreshTranscript()
    }
    popupContent.forceActiveFocus()
  }

  function refreshTranscript() {
    if (!expandedPaneId || !opened || transcriptProcess.running) return
    transcriptProcess.requestedPaneId = expandedPaneId
    transcriptProcess.command = [binaryPath, "--bar-transcript", socketPath, expandedPaneId]
    transcriptProcess.running = true
  }

  readonly property string navigationPath: decodeURIComponent(Qt.resolvedUrl("navigate.py").toString().replace(/^file:\/\//, ""))
  readonly property int refreshIntervalSec: Math.max(2, Number(setting("refreshIntervalSec", 5)) || 5)
  readonly property bool compact: String(setting("display", "Icon")).toLowerCase() === "compact"
  readonly property color foreground: bar ? bar.foreground : Color.foreground
  readonly property color accent: Color.accent
  readonly property color urgent: bar ? bar.urgent : Color.urgent

  property var snapshot: Model.emptySnapshot()
  property bool connected: false
  property bool loading: false
  property string dashboardPaneId: ""
  property string pendingPaneAction: ""

  readonly property var summary: Model.summarize(snapshot)
  readonly property color statusColor: Model.stateColor(summary, accent, urgent, foreground)
  readonly property string tooltipText: Model.tooltip(summary, connected)

  implicitWidth: compact && !vertical
    ? contents.implicitWidth + Style.space(14)
    : Style.bar.iconSlot
  implicitHeight: Style.bar.iconSlot

  FileView {
    id: themeColors
    path: Color.currentThemePath + "/colors.toml"
    watchChanges: true
    onFileChanged: reload()
    onLoaded: {
      var palette = {foreground: Color.foreground, muted: Color.muted, cyan: Color.accent, green: Color.accent, yellow: Color.foreground, red: Color.urgent, magenta: Color.accent}
      var aliases = {color2: "green", color3: "yellow", color6: "cyan", color1: "red", color5: "magenta", color7: "foreground", color8: "muted"}
      var regex = /^\s*(green|yellow|cyan|red|magenta|foreground|muted|color[1235678])\s*=\s*["']([^"']+)["']/gm
      var match, raw = text()
      while ((match = regex.exec(raw)) !== null) palette[aliases[match[1]] || match[1]] = match[2]
      root.headerPalette = palette
      root.wordmarkColor = palette.green
    }
  }

  function refresh() {
    if (!snapshotProcess.running) snapshotProcess.running = true
  }

  function consumeSnapshot(raw) {
    var parsed = Model.parseSnapshot(raw)
    if (parsed === null) return
    snapshot = parsed
    if (expandedPaneId && !expandedAgent) expandedPaneId = ""
    connected = parsed.connected === true
    if (!connected) errorText = "Herdr unavailable — check the socket setting"
    else if (errorText === "Herdr unavailable — check the socket setting") errorText = ""
  }

  function navigationCommand(action, paneId) {
    var command = ["env", "HERDR_SOCKET_PATH=" + socketPath, "python3", navigationPath, action]
    if (paneId) command.push(paneId)
    return command
  }

  function beginAgentAction(action, targetPaneId) {
    if (actionProcess.running || actionState || promptPaneId) return
    var paneId = targetPaneId || root.expandedPaneId || root.selectedPaneId || ((root.snapshot.agents || [])[0] || {}).pane_id
    if (!paneId || !connected) return
    errorText = ""
    actionState = {phase: "running", title: "Corgi", text: "Checking agent…"}
    actionProcess.dismissed = false
    actionProcess.command = [actionBinaryPath, "--bar-action", socketPath, paneId, action]
    actionProcess.running = true
  }

  function dismissAction() {
    if (!actionState) return
    actionProcess.dismissed = true
    if (actionProcess.running && actionState.phase !== "done") actionProcess.write("escape\n")
    actionState = null
  }

  function answerAction(key) {
    if (key === "escape") { dismissAction(); popupContent.forceActiveFocus(); return }
    if (!actionState || actionState.phase === "running") return
    if (actionState.phase === "done") actionState = null
    else actionProcess.write(key + "\n")
    popupContent.forceActiveFocus()
  }

  Process {
    id: actionProcess
    property bool dismissed: false
    stdinEnabled: true
    stdout: SplitParser {
      onRead: function(line) {
        if (actionProcess.dismissed) return
        try { root.actionState = JSON.parse(line) } catch (error) { root.errorText = "Invalid action response" }
      }
    }
    stderr: StdioCollector {
      onStreamFinished: if (!actionProcess.dismissed && text.trim()) root.actionState = {phase: "done", title: "Action unavailable", text: text.trim()}
    }
    onExited: function(exitCode) {
      if (!actionProcess.dismissed && root.actionState && root.actionState.phase === "running")
        root.actionState = {phase: "done", title: "Action failed", text: "Could not run this agent action"}
    }
  }

  function focusAgent(paneId) {
    if (focusProcess.running || paneProcess.running) return
    errorText = ""
    focusProcess.command = navigationCommand("agent", paneId)
    focusProcess.running = true
  }

  function openDashboard() {
    if (paneProcess.running || focusProcess.running) return
    errorText = ""
    pendingPaneAction = dashboardPaneId !== "" ? "focus" : "open"
    paneProcess.command = navigationCommand(pendingPaneAction, dashboardPaneId)
    paneProcess.running = true
  }

  Timer {
    interval: root.refreshIntervalSec * 1000
    running: true
    repeat: true
    triggeredOnStart: true
    onTriggered: root.refresh()
  }

  Timer {
    interval: root.refreshIntervalSec * 1000
    running: root.opened && root.expandedPaneId !== ""
    repeat: true
    onTriggered: root.refreshTranscript()
  }

  Process {
    id: transcriptProcess
    property string requestedPaneId: ""
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        try {
          var result = JSON.parse(text)
          if (result.pane_id === root.expandedPaneId && Array.isArray(result.entries)) root.transcriptEntries = result.entries
        } catch (error) {
          if (transcriptProcess.requestedPaneId === root.expandedPaneId) root.errorText = "Could not read this session's transcript"
        }
      }
    }
    stderr: StdioCollector {
      onStreamFinished: if (text.trim() && transcriptProcess.requestedPaneId === root.expandedPaneId) root.errorText = text.trim().slice(0, 240)
    }
    onExited: Qt.callLater(function() {
      if (root.expandedPaneId && root.expandedPaneId !== transcriptProcess.requestedPaneId) root.refreshTranscript()
    })
  }

  Process {
    id: snapshotProcess
    command: [root.binaryPath, "--bar-stream", root.socketPath]
    stdout: SplitParser { onRead: function(line) { root.consumeSnapshot(line) } }
    stderr: StdioCollector { onStreamFinished: { if (text.trim()) root.errorText = text.trim().slice(0, 240) } }
    onExited: function(exitCode) {
      root.connected = false
      root.errorText = "Corgi reader stopped. Check the binary and socket settings."
    }
  }

  Process {
    id: promptProcess
    stderr: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        if (!text.trim()) return
        try { root.errorText = JSON.parse(text).error.message || text.trim().slice(0, 240) }
        catch (error) { root.errorText = text.trim().slice(0, 240) }
      }
    }
    onExited: function(exitCode) {
      if (exitCode === 0) {
        root.noticeText = "Prompt sent"
        root.cancelPrompt()
        root.refresh()
        root.refreshTranscript()
      } else {
        if (!root.errorText) root.errorText = "Could not send prompt"
        if (root.opened) promptInput.forceActiveFocus()
      }
    }
  }

  Process {
    id: focusProcess
    stderr: StdioCollector { onStreamFinished: { if (text.trim()) root.errorText = text.trim().slice(0, 240) } }
    onExited: function(exitCode) {
      if (exitCode === 0) root.close()
      else if (!root.errorText) root.errorText = "Could not focus that agent"
    }
  }

  Process {
    id: paneProcess
    command: []
    stderr: StdioCollector { onStreamFinished: { if (text.trim()) root.errorText = text.trim().slice(0, 240) } }

    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var paneId = Model.paneIdFromResponse(text)
        if (paneId !== "") root.dashboardPaneId = paneId
      }
    }

    onExited: function(exitCode) {
      // A stale pane ID can occur after Herdr closes or changes session. The
      // next click then opens a fresh dashboard instead of repeating a failure.
      if (exitCode === 3 && root.pendingPaneAction === "focus") root.dashboardPaneId = ""
      if (exitCode === 0) root.close()
      else if (!root.errorText) root.errorText = "Could not open the dashboard"
      root.pendingPaneAction = ""
    }
  }

  Item {
    id: contents
    anchors.centerIn: parent
    implicitWidth: icon.width
      + (root.compact && !root.vertical ? label.implicitWidth + Style.space(6) : 0)
    implicitHeight: Math.max(icon.height, root.compact && !root.vertical ? label.implicitHeight : 0)
    width: implicitWidth
    height: implicitHeight

    Item {
      id: icon
      width: Style.bar.iconCanvas
      height: Style.bar.iconCanvas
      anchors.verticalCenter: parent.verticalCenter

      Image {
        id: barMascot
        anchors.fill: parent
        source: Qt.resolvedUrl("assets/corgi-pixel.svg")
        sourceSize.width: Math.round(width * Screen.devicePixelRatio)
        sourceSize.height: Math.round(height * Screen.devicePixelRatio)
        fillMode: Image.PreserveAspectFit
        smooth: false
        opacity: root.connected ? 1 : 0.5
        transform: Scale {
          origin.x: barMascot.width / 2
          xScale: root.mascotWidthScale
        }
      }

      Rectangle {
        // Use the icon slot's spare space so status never covers the mascot.
        visible: root.connected && (root.summary.working > 0 || root.summary.blocked > 0 || root.summary.done > 0)
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.top: parent.bottom
        anchors.topMargin: Style.space(1)
        width: Math.round(parent.width * 0.65)
        height: Math.max(1, Style.space(2))
        radius: height / 2
        color: root.summary.blocked > 0 ? root.urgent
          : root.summary.working > 0 ? root.accent : root.headerPalette.green
        opacity: root.summary.blocked > 0 ? 1 : 0.75
      }
    }

    Text {
      id: label
      visible: root.compact && !root.vertical
      anchors.left: icon.right
      anchors.leftMargin: Style.space(6)
      anchors.verticalCenter: parent.verticalCenter
      text: Model.compactText(root.summary, root.connected)
      textFormat: Text.PlainText
      font.family: root.bar ? root.bar.fontFamily : Style.font.family
      font.pixelSize: Style.font.caption || Style.space(11)
      font.weight: root.summary.working > 0 || root.summary.blocked > 0 ? Font.DemiBold : Font.Normal
      color: root.connected ? root.statusColor : root.foreground
      elide: Text.ElideRight
    }
  }

  MouseArea {
    id: barClick
    Component.onCompleted: if (root.bar && root.bar.registerClickTarget) root.bar.registerClickTarget(barClick)
    Component.onDestruction: if (root.bar && root.bar.unregisterClickTarget) root.bar.unregisterClickTarget(barClick)
    anchors.fill: parent
    acceptedButtons: Qt.LeftButton | Qt.RightButton
    hoverEnabled: true
    cursorShape: Qt.PointingHandCursor

    onClicked: function(mouse) {
      if (mouse.button === Qt.RightButton) root.refresh()
      else { root.toggle(); root.refresh() }
    }
    onEntered: if (root.bar) root.bar.showTooltip(root, root.tooltipText)
    onExited: if (root.bar) root.bar.hideTooltip(root)
  }

  KeyboardPanel {
    id: popup
    focusTarget: root.newAgentOpen ? newAgentDialog : root.promptPaneId ? promptInput : popupContent
    anchorItem: root
    bar: root.bar
    owner: root
    open: root.opened
    contentWidth: fittedContentWidth(Style.space(800))
    readonly property real collapsedHeight: Style.space(Math.min(560, 260 + root.summary.total * 65 + (Model.dashboardRows(root.snapshot).length - root.summary.total) * 20)) + (root.promptPaneId ? promptEditor.implicitHeight + Style.space(10) : 0)
    contentHeight: root.newAgentOpen ? cappedContentHeight(headerRow.implicitHeight + newAgentDialog.implicitHeight + footerRow.implicitHeight + Style.space(30)) : root.expandedPaneId === "" ? cappedContentHeight(collapsedHeight)
      : Math.max(cappedContentHeight(collapsedHeight), fittedContentHeight(headerRow.implicitHeight + footerRow.implicitHeight + (root.promptPaneId ? promptEditor.implicitHeight + Style.space(10) : 0) + latestReplyMeasure.implicitHeight + (root.latestReplyIndex > 0 ? newestEntryMeasure.implicitHeight : 0) + agentsBox.lineHeight * (Math.max(0, root.latestReplyIndex - 1) * 2 + 8) + Style.space(20)))

    Text {
      id: latestReplyMeasure
      visible: false
      width: Math.max(1, agentScroll.availableWidth - Style.space(64) - agentsBox.cellWidth * 3)
      text: root.expandedEntries.length ? root.expandedEntries[root.latestReplyIndex].text : ""
      textFormat: Text.PlainText
      font.family: root.fontFamily
      font.pixelSize: Style.space(13)
      wrapMode: Text.Wrap
    }
    Text {
      id: newestEntryMeasure
      visible: false
      width: latestReplyMeasure.width
      text: root.expandedEntries.length ? root.expandedEntries[0].text : ""
      textFormat: Text.PlainText
      font: latestReplyMeasure.font
      wrapMode: Text.Wrap
    }

    MouseArea {
      parent: popupContent.parent
      anchors.fill: parent
      z: 100
      enabled: root.opened && (root.actionState !== null || root.promptPaneId !== "")
      acceptedButtons: Qt.AllButtons
      onPressed: function(mouse) {
        var target = root.actionState ? actionDialog : promptEditor
        var point = mapToItem(target, mouse.x, mouse.y)
        if (point.x >= 0 && point.y >= 0 && point.x < target.width && point.y < target.height) {
          mouse.accepted = false
          return
        }
        if (root.actionState) root.dismissAction()
        else { root.promptPaneId = ""; promptInput.text = "" }
        popupContent.forceActiveFocus()
      }
    }

    ColumnLayout {
      id: popupContent
      anchors.fill: parent
      spacing: Style.space(10)
      Keys.onPressed: function(event) {
        if (root.newAgentOpen) return
        if (!root.actionState && !root.promptPaneId && event.key === Qt.Key_N && event.modifiers === Qt.NoModifier) {
          root.openNewAgent(); event.accepted = true; return
        }
        if (!root.actionState && !root.promptPaneId && event.key === Qt.Key_P && event.modifiers === Qt.NoModifier) {
          root.beginPrompt(); event.accepted = true; return
        }
        if (root.promptPaneId) return
        if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
          if (root.actionState) { root.answerAction("enter"); event.accepted = true }
        } else if (event.key === Qt.Key_X || event.key === Qt.Key_M) {
          if (root.actionState) {
            if (event.key === Qt.Key_X && root.actionState.phase === "succeeded") root.answerAction("x")
          } else root.beginAgentAction(event.key === Qt.Key_X ? "x" : "m")
          event.accepted = true
        }
      }
      Keys.onSpacePressed: function(event) {
        if (root.newAgentOpen) return
        if (root.actionState) { event.accepted = true; return }
        if (root.promptPaneId) return
        var paneId = root.expandedPaneId || root.selectedPaneId || ((root.snapshot.agents || [])[0] || {}).pane_id
        if (paneId) root.toggleAgent(paneId)
        event.accepted = true
      }
      Keys.onEscapePressed: function(event) {
        if (root.newAgentOpen) root.newAgentOpen = false
        else if (root.actionState) root.answerAction("escape")
        else if (root.promptPaneId) root.cancelPrompt()
        else if (root.expandedPaneId) root.expandedPaneId = ""
        else root.close()
        event.accepted = true
      }
      RowLayout {
        id: headerRow
        Layout.fillWidth: true
        Image {
          id: popupMascot
          smooth: false
          source: Qt.resolvedUrl("assets/corgi-pixel.svg")
          Layout.preferredWidth: herdCard.implicitHeight * 16 / 14
          Layout.preferredHeight: herdCard.implicitHeight
          Layout.minimumWidth: Style.space(40)
          fillMode: Image.PreserveAspectFit
          transform: Scale {
            origin.x: popupMascot.width / 2
            xScale: root.mascotWidthScale
          }
          MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onClicked: root.openDashboard()
          }
        }
        Image {
          smooth: false
          source: Qt.resolvedUrl("assets/corgi-wordmark.svg")
          Layout.preferredWidth: herdCard.implicitHeight * 5 / 7 * 38 / 10
          Layout.preferredHeight: herdCard.implicitHeight * 5 / 7
          Layout.minimumWidth: Style.space(95)
          fillMode: Image.PreserveAspectFit
          layer.enabled: true
          layer.effect: MultiEffect { colorization: 1; colorizationColor: root.wordmarkColor }
          MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onClicked: root.openDashboard()
          }
        }
        HeaderCard {
          id: herdCard
          // One extra blank character before the first box.
          Layout.leftMargin: cellWidth
          title: "HERD"
          columns: 15
          horizontalPadding: 0
          palette: root.headerPalette
          frameColor: root.summary.blocked > 0 ? palette.yellow : root.summary.working > 0 ? palette.cyan : root.summary.done > 0 ? palette.green : palette.muted
          lines: Model.herdLines(root.summary)
        }
        Repeater {
          model: root.snapshot.usage || []
          HeaderCard {
            required property var modelData
            title: String(modelData.provider || "").toUpperCase()
            palette: root.headerPalette
            frameColor: modelData.provider === "Codex" ? "#00afff" : "#ff8700"
            lines: modelData.lines || []
          }
        }
        // A provider whose CLI is installed but whose usage cannot be read
        // gets no card, so its reason sits in the header's top right corner,
        // the same as in the dashboard.
        Item {
          Layout.fillWidth: true
          Layout.fillHeight: true
          ColumnLayout {
            anchors.right: parent.right
            anchors.top: parent.top
            spacing: 0
            Repeater {
              model: root.snapshot.usage_notes || []
              RowLayout {
                required property var modelData
                Layout.alignment: Qt.AlignRight
                spacing: 0
                Text {
                  text: "⚠ " + String(modelData.provider || "").toUpperCase() + " "
                  textFormat: Text.PlainText
                  color: root.headerPalette.yellow
                  font.bold: true
                  font.family: root.fontFamily
                  font.pixelSize: Style.space(12)
                }
                Text {
                  text: String(modelData.reason || "")
                  textFormat: Text.PlainText
                  color: root.headerPalette.muted
                  elide: Text.ElideRight
                  font.family: root.fontFamily
                  font.pixelSize: Style.space(12)
                }
              }
            }
          }
        }
      }
      Text {
        Layout.fillWidth: true
        visible: root.errorText !== ""
        text: root.errorText
        textFormat: Text.PlainText
        color: Color.urgent
        wrapMode: Text.Wrap
        font.family: root.fontFamily
      }
      Item {
        id: agentsBox
        visible: !root.newAgentOpen
        Layout.fillWidth: true
        Layout.fillHeight: true
        readonly property real cellWidth: agentMetrics.advanceWidth("0")
        readonly property real lineHeight: Math.ceil(agentMetrics.height)
        FontMetrics { id: agentMetrics; font.family: root.fontFamily; font.pixelSize: Style.space(13) }
        Rectangle {
          anchors.fill: parent
          anchors.topMargin: agentsBox.lineHeight / 2
          anchors.bottomMargin: -agentsBox.lineHeight / 2
          radius: Style.space(3)
          color: "transparent"
          border.width: 1
          border.color: root.headerPalette.foreground
        }
        Rectangle {
          x: agentsBox.cellWidth
          width: agentsTitle.implicitWidth
          height: agentsBox.lineHeight
          color: Color.popups.background
          Text {
            id: agentsTitle
            text: " Agents "
            textFormat: Text.PlainText
            color: root.headerPalette.foreground
            font.family: root.fontFamily
            font.pixelSize: Style.space(13)
          }
        }
        ScrollView {
          id: agentScroll
          anchors.fill: parent
          anchors.topMargin: agentsBox.lineHeight
          anchors.leftMargin: Style.space(2)
          anchors.rightMargin: 1
          contentWidth: availableWidth
          clip: true
          ScrollBar.horizontal.policy: ScrollBar.AlwaysOff
          Column {
            width: agentScroll.availableWidth
            spacing: 0
            Text {
              visible: root.connected && root.summary.total === 0
              width: parent.width
              text: "No coding agents are currently detected in this Herdr session.\n\nClick New agent below to start one."
              horizontalAlignment: Text.AlignHCenter
              wrapMode: Text.Wrap
              textFormat: Text.PlainText
              color: root.headerPalette.muted
              font.family: root.fontFamily
              font.pixelSize: Style.space(13)
            }
            Repeater {
              model: root.rows
              delegate: Rectangle {
                id: row
                required property var modelData
                required property int index
                width: parent.width
                implicitHeight: Math.max(rowContent.implicitHeight, sessionActions.visible ? sessionActions.implicitHeight : 0) + (modelData.heading ? 0 : agentsBox.lineHeight)
                readonly property bool expanded: !modelData.heading && modelData.pane_id === root.expandedPaneId
                color: (rowMouse.containsMouse || expanded) && !modelData.heading ? Style.selectedFillFor(Color.foreground, Color.accent) : "transparent"
                Rectangle {
                  visible: (rowMouse.containsMouse || row.expanded) && !row.modelData.heading
                  width: Style.space(2)
                  height: rowContent.implicitHeight
                  color: root.headerPalette.cyan
                }
                ColumnLayout {
                  id: rowContent
                  clip: true
                  z: 1
                  anchors.left: parent.left
                  anchors.right: parent.right
                  anchors.top: parent.top
                  anchors.leftMargin: row.modelData.heading ? 0 : Style.space(4)
                  anchors.rightMargin: row.modelData.heading ? 0 : agentsBox.cellWidth - 1
                  spacing: 0
                  Item {
                    visible: row.modelData.heading === true
                    Layout.fillWidth: true
                    Layout.topMargin: row.index > 0 ? agentsBox.lineHeight : 0
                    implicitHeight: agentsBox.lineHeight
                    Rectangle {
                      anchors.left: parent.left
                      anchors.right: parent.right
                      anchors.verticalCenter: parent.verticalCenter
                      height: 1
                      color: root.headerPalette.muted
                    }
                    Rectangle {
                      // Start the name beneath the middle of the Agents title.
                      x: agentsBox.cellWidth / 2 + agentsTitle.implicitWidth / 2 - Style.space(2)
                      width: Math.max(0, Math.min(projectName.implicitWidth + agentsBox.cellWidth, parent.width - x))
                      height: agentsBox.lineHeight
                      color: Color.popups.background
                      clip: true
                      Text {
                        id: projectName
                        anchors.fill: parent
                        anchors.leftMargin: agentsBox.cellWidth / 2
                        anchors.rightMargin: agentsBox.cellWidth / 2
                        text: row.modelData.project || ""
                        textFormat: Text.PlainText
                        color: root.headerPalette.green
                        font.family: root.fontFamily
                        font.pixelSize: Style.space(13)
                        font.bold: true
                        elide: Text.ElideRight
                      }
                    }
                  }
                  Row {
                    id: statusRow
                    Layout.rightMargin: closeSessionButton.width + Style.space(8)
                    visible: !row.modelData.heading
                    Layout.fillWidth: true
                    Layout.minimumWidth: 0
                    spacing: Style.space(6)
                    Rectangle {
                      id: statusBadge
                      visible: !row.modelData.heading
                      implicitWidth: statusText.implicitWidth + Style.space(8)
                      implicitHeight: statusText.implicitHeight
                      color: Model.agentColor(row.modelData, root.headerPalette.cyan, root.headerPalette.yellow, root.headerPalette.magenta, root.headerPalette.muted, root.headerPalette.green)
                      Text {
                        id: statusText
                        anchors.centerIn: parent
                        text: Model.stateFor(row.modelData).toUpperCase()
                        textFormat: Text.PlainText
                        color: Color.background
                        font.family: root.fontFamily
                        font.bold: true
                        font.pixelSize: Style.space(13)
                      }
                    }
                    Text {
                      id: statusTask
                      width: Math.min(implicitWidth, Math.max(0, statusRow.width - statusBadge.width - Math.min(statusDetails.implicitWidth, statusRow.width * 0.65) - statusRow.spacing * 2))
                      text: Model.singleLine(row.modelData.task || row.modelData.name || "No task yet")
                      textFormat: Text.PlainText
                      color: Color.foreground
                      font.family: root.fontFamily
                      font.bold: rowMouse.containsMouse
                      font.pixelSize: Style.space(13)
                      elide: Text.ElideRight
                    }
                    Text {
                      id: statusDetails
                      clip: true
                      visible: !row.modelData.heading
                      width: Math.min(implicitWidth, Math.max(0, statusRow.width - statusBadge.width - statusTask.width - statusRow.spacing * 2))
                      text: Model.detailsHtml(row.modelData, root.headerPalette)
                      textFormat: Text.RichText
                      color: root.headerPalette.muted
                      font.family: root.fontFamily
                      font.pixelSize: Style.space(13)
                      elide: Text.ElideRight
                    }

                  }
                  Text {
                    Layout.fillWidth: true
                    visible: !row.modelData.heading && !row.expanded
                    Layout.rightMargin: promptSessionButton.width + Style.space(8)
                    text: Model.activityMarker(row.modelData.message_kind, "›") + " " + Model.singleLine(row.modelData.message || "No message yet")
                    textFormat: Text.PlainText
                    color: Model.activityColor(row.modelData.message_kind, root.headerPalette)
                    font.family: root.fontFamily
                    font.pixelSize: Style.space(13)
                    elide: Text.ElideRight
                  }
                  Column {
                    Layout.fillWidth: true
                    Layout.rightMargin: sessionActions.width + Style.space(8)
                    visible: row.expanded
                    Repeater {
                      model: row.expanded ? root.expandedEntries : []
                      Row {
                        required property var modelData
                        required property int index
                        width: parent.width
                        readonly property bool full: index === 0 || index === root.latestReplyIndex
                        Text {
                          width: agentsBox.cellWidth * 2
                          text: Model.activityMarker(parent.modelData.kind, "›")
                          color: Model.activityColor(parent.modelData.kind, root.headerPalette)
                          font.family: root.fontFamily
                          font.pixelSize: Style.space(13)
                          font.bold: true
                        }
                        Text {
                          width: Math.max(1, parent.width - agentsBox.cellWidth * 2)
                          text: parent.modelData.text
                          textFormat: Text.PlainText
                          color: Model.activityColor(parent.modelData.kind, root.headerPalette)
                          font.family: root.fontFamily
                          font.pixelSize: Style.space(13)
                          wrapMode: Text.Wrap
                          maximumLineCount: parent.full ? 2147483647 : 2
                          elide: parent.full ? Text.ElideNone : Text.ElideRight
                          bottomPadding: parent.index === 0 ? agentsBox.lineHeight : 0
                        }
                      }
                    }
                  }
                  Text {
                    Layout.fillWidth: true
                    visible: !row.modelData.heading && !row.expanded
                    Layout.rightMargin: mergeSessionButton.width + Style.space(8)
                    text: Model.activityMarker(row.modelData.tool_kind, "●") + " " + Model.singleLine(row.modelData.tool || "No command yet")
                    textFormat: Text.PlainText
                    color: root.headerPalette.cyan
                    font.family: root.fontFamily
                    font.pixelSize: Style.space(13)
                    elide: Text.ElideRight
                  }
                }
                ColumnLayout {
                  id: sessionActions
                  z: 2
                  visible: !row.modelData.heading
                  anchors.top: parent.top
                  anchors.topMargin: Style.space(1.5)
                  anchors.right: parent.right
                  anchors.rightMargin: agentsBox.cellWidth - 1
                  width: implicitWidth
                  spacing: Style.space(3)
                  enabled: root.connected && !root.actionState && !actionProcess.running && !root.promptPaneId && !promptProcess.running
                  SessionActionButton {
                    id: closeSessionButton
                    Layout.alignment: Qt.AlignRight
                    Layout.preferredHeight: agentsBox.lineHeight - Style.space(3)
                    Layout.minimumHeight: 0
                    text: "X"
                    tone: root.headerPalette.red
                    Accessible.name: "Close this session"
                    onClicked: root.beginAgentAction("x", row.modelData.pane_id)
                  }
                  SessionActionButton {
                    id: promptSessionButton
                    Layout.alignment: Qt.AlignRight
                    Layout.preferredHeight: agentsBox.lineHeight - Style.space(3)
                    Layout.minimumHeight: 0
                    text: "Prompt"
                    tone: root.headerPalette.cyan
                    Accessible.name: "Prompt this session"
                    enabled: Model.stateFor(row.modelData) !== "blocked"
                    onClicked: root.beginPrompt(row.modelData.pane_id)
                  }
                  SessionActionButton {
                    id: mergeSessionButton
                    Layout.alignment: Qt.AlignRight
                    Layout.preferredHeight: agentsBox.lineHeight - Style.space(3)
                    Layout.minimumHeight: 0
                    text: "Merge"
                    tone: root.headerPalette.green
                    Accessible.name: "Merge this session"
                    onClicked: root.beginAgentAction("m", row.modelData.pane_id)
                  }
                }
                MouseArea {
                  id: rowMouse
                  anchors.fill: parent
                  enabled: !row.modelData.heading && root.connected && !root.actionState
                  hoverEnabled: true
                  acceptedButtons: Qt.LeftButton | Qt.RightButton
                  cursorShape: Qt.PointingHandCursor
                  onEntered: root.selectedPaneId = row.modelData.pane_id
                  onClicked: function(mouse) {
                    if (mouse.button === Qt.RightButton) root.focusAgent(row.modelData.pane_id)
                    else root.toggleAgent(row.modelData.pane_id)
                  }
                }
              }
            }
          }
        }
      }
      NewAgentDialog {
        id: newAgentDialog
        visible: root.newAgentOpen
        Layout.fillWidth: true
        binaryPath: root.actionBinaryPath
        socketPath: root.socketPath
        paneId: root.expandedPaneId || root.selectedPaneId
        palette: root.headerPalette
        fontFamily: root.fontFamily
        onDismissed: { root.newAgentOpen = false; popupContent.forceActiveFocus() }
        onCreated: function(message) { root.noticeText = message; root.close() }
      }
      ActionDialog {
        id: actionDialog
        Layout.fillWidth: true
        state: root.actionState
        palette: root.headerPalette
        fontFamily: root.fontFamily
        onAnswered: function(key) { root.answerAction(key) }
      }
      ColumnLayout {
        id: promptEditor
        visible: root.promptPaneId !== ""
        Layout.fillWidth: true
        spacing: Style.space(6)
        Text {
          Layout.fillWidth: true
          text: "Prompt → " + root.promptLabel
          textFormat: Text.PlainText
          elide: Text.ElideRight
          color: root.headerPalette.cyan
          font.family: root.fontFamily
          font.pixelSize: Style.space(13)
        }
        ScrollView {
          Layout.fillWidth: true
          Layout.preferredHeight: Style.space(90)
          clip: true
          TextArea {
            id: promptInput
            readOnly: promptProcess.running
            placeholderText: "Write a prompt…"
            wrapMode: TextEdit.Wrap
            color: Color.foreground
            font.family: root.fontFamily
            font.pixelSize: Style.space(13)
            background: Rectangle {
              color: Color.background
              radius: Style.space(3)
              border.color: promptInput.activeFocus ? root.headerPalette.cyan : root.headerPalette.muted
            }
            Keys.onPressed: function(event) {
              if ((event.key === Qt.Key_Return || event.key === Qt.Key_Enter) && !(event.modifiers & Qt.ShiftModifier)) {
                root.submitPrompt()
                event.accepted = true
              } else if (event.key === Qt.Key_Escape) {
                root.cancelPrompt()
                event.accepted = true
              }
            }
          }
        }
        RowLayout {
          Layout.fillWidth: true
          Text {
            text: "Enter: send · Shift+Enter: newline · Esc: cancel"
            color: root.headerPalette.muted
            font.family: root.fontFamily
            font.pixelSize: Style.space(12)
            Layout.fillWidth: true
          }
          Button { text: "Cancel"; enabled: !promptProcess.running; onClicked: root.cancelPrompt() }
          Button {
            text: promptProcess.running ? "Sending…" : "Send"
            enabled: !promptProcess.running && promptInput.text.trim() !== ""
            onClicked: root.submitPrompt()
          }
        }
      }
      RowLayout {
        id: footerRow
        Text { text: root.noticeText || "Click: expand · Right click: focus · p / m / x: actions"; textFormat: Text.PlainText; color: root.noticeText ? root.headerPalette.green : Color.foreground
          opacity: 0.8; font.family: root.fontFamily; font.pixelSize: Style.space(12); Layout.fillWidth: true }
        Button { text: "New agent +"; enabled: !newAgentDialog.busy && !root.actionState && !root.promptPaneId; onClicked: root.openNewAgent() }
        Button { text: "Open dashboard ↗"; onClicked: root.openDashboard() }
      }
    }
  }
}
