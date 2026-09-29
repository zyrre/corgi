import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import qs.Commons
import "Model.js" as Model

Rectangle {
  id: dialog
  required property var state
  required property var palette
  required property string fontFamily
  signal answered(string key)
  readonly property color toneColor: palette[(state || {}).tone || "cyan"] || palette.foreground
  readonly property string phase: (state || {}).phase || "done"
  visible: state !== null
  implicitHeight: content.implicitHeight + Style.space(32)
  color: Color.popups.background
  border.color: toneColor
  border.width: Style.space(1)
  radius: Style.space(6)

  function richLine(line) {
    return (line.spans || []).map(function(span) {
      var text = Model.escapeHtml(span.text).replace(/ {2}/g, "&nbsp;&nbsp;").replace(/\n/g, "<br>")
      return '<span style="color:' + Model.escapeHtml(dialog.palette[span.tone] || dialog.palette.foreground) + '">' + (span.bold ? '<b>' + text + '</b>' : text) + '</span>'
    }).join("")
  }

  ColumnLayout {
    id: content
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.top: parent.top
    anchors.margins: Style.space(16)
    spacing: Style.space(12)
    RowLayout {
      Layout.alignment: Qt.AlignHCenter
      spacing: Style.space(8)
      Text {
        text: dialog.phase === "running" ? "◌" : dialog.phase === "succeeded" ? "✓" : dialog.state && dialog.state.tone === "red" ? "!" : "⑂"
        color: dialog.toneColor
        font.family: dialog.fontFamily
        font.bold: true
        font.pixelSize: Style.space(20)
      }
      Text {
        text: (dialog.state || {}).title || "Corgi"
        textFormat: Text.PlainText
        color: dialog.toneColor
        font.family: dialog.fontFamily
        font.bold: true
        font.pixelSize: Style.space(15)
        elide: Text.ElideRight
        Layout.maximumWidth: Math.max(1, dialog.width - Style.space(90))
      }
    }
    Rectangle {
      Layout.fillWidth: true
      implicitHeight: 1
      color: dialog.toneColor
      opacity: 0.35
    }
    ScrollView {
      id: scroll
      Layout.fillWidth: true
      Layout.preferredHeight: Math.min(body.implicitHeight, Style.space(230))
      clip: true
      contentWidth: availableWidth
      Column {
        id: body
        width: scroll.availableWidth
        spacing: 0
        Repeater {
          model: dialog.state && dialog.state.lines ? dialog.state.lines : [{spans: [{text: (dialog.state || {}).text || "", tone: "foreground"}]}]
          Text {
            required property var modelData
            width: body.width
            text: dialog.richLine(modelData)
            textFormat: Text.RichText
            horizontalAlignment: modelData.left ? Text.AlignLeft : Text.AlignHCenter
            wrapMode: Text.Wrap
            color: dialog.palette.foreground
            font.family: dialog.fontFamily
            font.pixelSize: Style.space(13)
            // Preserve the dashboard's empty lines and paragraph spacing.
            height: Math.max(implicitHeight, Style.space(17))
          }
        }
      }
    }
    RowLayout {
      Layout.alignment: Qt.AlignHCenter
      spacing: Style.space(12)
      Button {
        id: confirmButton
        visible: ["confirm", "failed", "succeeded"].indexOf(dialog.phase) >= 0
        text: dialog.phase === "succeeded" ? "x  Close worktree" : dialog.phase === "failed" ? "↻  Retry" : "↵  " + ((dialog.state || {}).confirm || "Confirm")
        onClicked: dialog.answered(dialog.phase === "succeeded" ? "x" : "enter")
        contentItem: Text {
          text: confirmButton.text
          color: Color.background
          font.family: dialog.fontFamily
          font.pixelSize: Style.space(12)
          font.bold: true
          horizontalAlignment: Text.AlignHCenter
          verticalAlignment: Text.AlignVCenter
        }
        background: Rectangle {
          implicitWidth: Style.space(140)
          implicitHeight: Style.space(32)
          radius: Style.space(4)
          color: dialog.toneColor
          opacity: confirmButton.down ? 0.7 : confirmButton.hovered ? 0.85 : 1
        }
      }
      Button {
        id: cancelButton
        enabled: true
        text: dialog.phase === "succeeded" ? "Keep open" : ["done", "running"].indexOf(dialog.phase) >= 0 ? "Dismiss" : "Esc  Cancel"
        onClicked: dialog.answered("escape")
        contentItem: Text {
          text: cancelButton.text
          color: dialog.palette.muted
          font.family: dialog.fontFamily
          font.pixelSize: Style.space(12)
          horizontalAlignment: Text.AlignHCenter
          verticalAlignment: Text.AlignVCenter
        }
        background: Rectangle {
          implicitWidth: Style.space(100)
          implicitHeight: Style.space(32)
          radius: Style.space(4)
          color: cancelButton.hovered ? Color.background : "transparent"
          border.color: dialog.palette.muted
          opacity: cancelButton.enabled ? 0.7 : 0.3
        }
      }
    }
  }
}
