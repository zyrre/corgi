import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell.Io
import qs.Commons

Rectangle {
  id: dialog
  required property string binaryPath
  required property string socketPath
  required property string paneId
  required property var palette
  required property string fontFamily
  signal dismissed()
  signal created(string message)
  property var options: ({harnesses: [], models: [], efforts: [], projects: []})
  property string statusText: ""
  property string phase: ""
  property bool initialOptions: true
  property string requestedModel: ""
  property string requestedEffort: ""
  property string requestedProject: ""
  readonly property bool busy: launch.running
  implicitHeight: content.implicitHeight + Style.space(24)
  color: Color.popups.background
  radius: Style.space(6)
  border.color: palette.cyan

  function loadOptions(initial) {
    initialOptions = initial
    requestedModel = initial ? "" : model.currentIndex >= 0 && model.editText === model.currentText ? model.currentValue : model.editText
    requestedEffort = initial ? "" : effort.currentIndex >= 0 && effort.editText === effort.currentText ? effort.currentValue : effort.editText
    requestedProject = project.editText
    statusText = "Loading harness choices…"
    optionsProcess.command = [binaryPath, "--bar-new-options", socketPath, paneId, initial ? "" : harness.editText, requestedModel]
    optionsProcess.running = true
  }
  function begin() {
    task.text = ""
    phase = ""
    checkout.currentIndex = 0
    loadOptions(true)
    task.forceActiveFocus()
  }
  function submit() {
    if (busy || optionsProcess.running || phase === "succeeded") return
    if (!task.text.trim() || !project.editText.trim()) { statusText = "Describe the task and choose a project."; phase = "failed"; return }
    phase = "running"
    statusText = "Starting new agent…"
    launch.command = [binaryPath, "--bar-new-agent", socketPath]
    launch.running = true
  }
  Process {
    id: optionsProcess
    stdout: StdioCollector {
      onStreamFinished: {
        try {
          var data = JSON.parse(text)
          dialog.options = data
          harness.currentIndex = harness.indexOfValue(data.kind)
          harness.editText = data.kind
          model.currentIndex = model.indexOfValue(dialog.requestedModel)
          if (model.currentIndex < 0) model.editText = dialog.requestedModel
          effort.currentIndex = Math.max(0, effort.indexOfValue(dialog.requestedEffort))
          project.editText = dialog.initialOptions ? data.project : dialog.requestedProject
          dialog.statusText = ""
        } catch (error) { dialog.statusText = "Could not load agent settings." }
      }
    }
    stderr: StdioCollector { onStreamFinished: if (text.trim()) dialog.statusText = text.trim() }
  }
  Process {
    id: launch
    stdinEnabled: true
    onStarted: {
      write(JSON.stringify({task: task.text, project: project.editText, kind: harness.editText,
        model: model.currentIndex >= 0 && model.editText === model.currentText ? model.currentValue : model.editText,
        effort: effort.currentIndex >= 0 && effort.editText === effort.currentText ? effort.currentValue : effort.editText,
        checkout: checkout.currentIndex === 0 ? "worktree" : "directory"}) + "\n")
    }
    stdout: SplitParser {
      onRead: function(line) {
        try {
          var state = JSON.parse(line)
          dialog.phase = state.phase
          dialog.statusText = state.text
          if (state.phase === "succeeded") dialog.created(state.text)
        } catch (error) { dialog.phase = "failed"; dialog.statusText = "Invalid launch response." }
      }
    }
    stderr: StdioCollector { onStreamFinished: if (text.trim()) { dialog.phase = "failed"; dialog.statusText = text.trim() } }
    onExited: function(code) { if (dialog.phase === "running") { dialog.phase = "failed"; dialog.statusText = "Could not start agent." } }
  }
  Keys.onEscapePressed: function(event) { dialog.dismissed(); event.accepted = true }
  ColumnLayout {
    id: content
    anchors.fill: parent
    anchors.margins: Style.space(12)
    spacing: Style.space(10)
    Text { text: "＋ New agent"; color: dialog.palette.cyan; font.family: dialog.fontFamily; font.bold: true; font.pixelSize: Style.space(16) }
    Text { text: "Task"; color: dialog.palette.foreground; font.family: dialog.fontFamily }
    ScrollView {
      Layout.fillWidth: true
      Layout.preferredHeight: Style.space(110)
      clip: true
      TextArea {
        id: task
        placeholderText: "Describe the first task…"
        wrapMode: TextEdit.Wrap
        readOnly: dialog.busy
        color: dialog.palette.foreground
        font.family: dialog.fontFamily
        background: Rectangle { color: Color.background; border.color: task.activeFocus ? dialog.palette.cyan : dialog.palette.muted; radius: Style.space(3) }
        Keys.onPressed: function(event) {
          if ((event.key === Qt.Key_Return || event.key === Qt.Key_Enter) && !(event.modifiers & Qt.ShiftModifier)) { dialog.submit(); event.accepted = true }
        }
      }
    }
    GridLayout {
      columns: 2
      Layout.fillWidth: true
      enabled: !dialog.busy && !optionsProcess.running && dialog.phase !== "succeeded"
      Text { text: "Harness"; color: dialog.palette.foreground; font.family: dialog.fontFamily }
      ComboBox {
        id: harness
        Layout.fillWidth: true
        font.family: dialog.fontFamily
        font.pixelSize: Style.space(13)
        editable: true
        textRole: "value"
        valueRole: "value"
        model: dialog.options.harnesses
        onActivated: { model.currentIndex = 0; effort.currentIndex = 0; dialog.loadOptions(false) }
        onAccepted: { model.currentIndex = 0; effort.currentIndex = 0; dialog.loadOptions(false) }
      }
      Text { text: "Model"; color: dialog.palette.foreground; font.family: dialog.fontFamily }
      ComboBox {
        id: model
        Layout.fillWidth: true
        font.family: dialog.fontFamily
        font.pixelSize: Style.space(13)
        editable: true
        textRole: "label"
        valueRole: "value"
        model: dialog.options.models
        onActivated: dialog.loadOptions(false)
        onAccepted: dialog.loadOptions(false)
      }
      Text { text: "Thinking effort"; visible: dialog.options.supportsEffort || false; color: dialog.palette.foreground; font.family: dialog.fontFamily }
      ComboBox {
        id: effort
        visible: dialog.options.supportsEffort || false
        Layout.fillWidth: true
        font.family: dialog.fontFamily
        font.pixelSize: Style.space(13)
        editable: true
        textRole: "label"
        valueRole: "value"
        model: dialog.options.efforts
      }
      Text { text: "Project"; color: dialog.palette.foreground; font.family: dialog.fontFamily }
      ComboBox { font.family: dialog.fontFamily; font.pixelSize: Style.space(13); id: project; Layout.fillWidth: true; editable: true; model: dialog.options.projects }
      Text { text: "Checkout"; color: dialog.palette.foreground; font.family: dialog.fontFamily }
      ComboBox { font.family: dialog.fontFamily; font.pixelSize: Style.space(13); id: checkout; Layout.fillWidth: true; model: ["New Git worktree", "Project directory as is"] }
    }
    Text {
      Layout.fillWidth: true
      visible: dialog.statusText !== ""
      text: dialog.statusText
      textFormat: Text.PlainText
      wrapMode: Text.Wrap
      color: dialog.phase === "failed" ? dialog.palette.red : dialog.palette.cyan
      font.family: dialog.fontFamily
      font.pixelSize: Style.space(12)
    }
    RowLayout {
      Layout.fillWidth: true
      Text { Layout.fillWidth: true; text: "Enter: create · Shift+Enter: newline · Esc: dismiss"; color: dialog.palette.muted; font.family: dialog.fontFamily; font.pixelSize: Style.space(12) }
      Button { text: dialog.busy ? "Dismiss" : "Cancel"; onClicked: dialog.dismissed() }
      Button { text: dialog.busy ? "Starting…" : "Create agent"; enabled: !dialog.busy && !optionsProcess.running && dialog.phase !== "succeeded"; onClicked: dialog.submit() }
    }
  }
}
