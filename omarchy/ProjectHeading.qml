import QtQuick
import QtQuick.Controls as Controls
import qs.Commons

// Whole state segments wrap together, so narrow popups keep every count and
// label legible. The top rule and its text remain within the scrolling list.
Item {
  id: root
  required property var project
  required property var palette
  required property real cellWidth
  required property real lineHeight
  property real leftInset: 0
  property string fontFamily: "monospace"
  implicitHeight: flow.implicitHeight
  Rectangle {
    width: parent.width
    y: root.lineHeight / 2
    height: 1
    color: root.palette.muted
  }
  Flow {
    id: flow
    x: root.leftInset
    width: Math.max(1, root.width - x)
    spacing: 0
    Repeater {
      model: [{text: (root.project.card ? (root.project.projectExpanded ? "▾ " : "▸ ") : "") + String(root.project.project || ""), tone: "green"}].concat(root.project.workerSegments || [])
      Rectangle {
        required property var modelData
        width: Math.min(flow.width, label.implicitWidth + root.cellWidth)
        height: Math.max(root.lineHeight, label.implicitHeight)
        color: Color.popups.background
        Accessible.name: modelData.label || modelData.text
        HoverHandler { id: hover }
        Controls.ToolTip.visible: hover.hovered
        Controls.ToolTip.delay: 500
        Controls.ToolTip.text: modelData.label || modelData.text
        Text {
          id: label
          x: root.cellWidth / 2
          width: Math.max(1, parent.width - root.cellWidth)
          text: modelData.text
          textFormat: Text.PlainText
          wrapMode: Text.Wrap
          color: root.palette[modelData.tone] || root.palette.foreground
          font.family: root.fontFamily
          font.pixelSize: Style.space(13)
          font.bold: true
        }
      }
    }
  }
}
