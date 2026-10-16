import QtQuick
import qs.Commons

// Same cell proportions, rounded frame, title, and five content rows as the TUI.
Item {
  id: root
  required property string title
  required property color frameColor
  required property var lines
  required property var palette
  property int columns: 14
  property int horizontalPadding: 1
  // advanceWidth() in a binding does not track font changes: it cached the
  // application's initial 15px font even after our 12px font was applied.
  // TextMetrics exposes a notifying property, so the box and text use one grid.
  readonly property real cellWidth: cellMetrics.advanceWidth
  readonly property real lineHeight: Math.ceil(metrics.height)
  implicitWidth: columns * cellWidth
  implicitHeight: lineHeight * 7

  FontMetrics { id: metrics; font.family: "monospace"; font.pixelSize: Style.space(12) }
  TextMetrics { id: cellMetrics; font: metrics.font; text: "0" }
  Rectangle {
    anchors.fill: parent
    anchors.topMargin: root.lineHeight / 2
    anchors.bottomMargin: root.lineHeight / 2
    radius: Style.space(3)
    color: "transparent"
    border.width: 1
    border.color: root.frameColor
  }
  Rectangle {
    x: root.cellWidth
    width: titleText.implicitWidth
    height: root.lineHeight
    color: Color.popups.background
    Text {
      id: titleText
      text: " " + root.title + " "
      textFormat: Text.PlainText
      color: root.frameColor
      font.family: "monospace"
      font.pixelSize: Style.space(12)
      font.bold: true
    }
  }
  Column {
    x: (1 + root.horizontalPadding) * root.cellWidth
    y: root.lineHeight
    width: root.width - 2 * (1 + root.horizontalPadding) * root.cellWidth
    Repeater {
      model: root.lines
      delegate: Item {
        id: line
        required property var modelData
        width: parent.width
        height: root.lineHeight
        clip: true
        Row {
          x: line.modelData.right ? line.width - implicitWidth : 0
          Repeater {
            model: line.modelData.spans
            Text {
              required property var modelData
              text: modelData.text
              // Terminal spans reserve cells even when a fallback glyph has a
              // different pixel advance (notably the merge arrow).
              width: modelData.cells === undefined ? implicitWidth : modelData.cells * root.cellWidth
              textFormat: Text.PlainText
              color: root.palette[modelData.tone] || Color.foreground
              font.family: "monospace"
              font.pixelSize: Style.space(12)
              font.bold: modelData.bold === true
            }
          }
        }
      }
    }
  }
}
