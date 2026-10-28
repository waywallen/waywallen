pragma ComponentBehavior: Bound
pragma ValueTypeBehavior: Assertable
import QtQuick
import QExtra as QE
import Qcm.Material as MD
import waywallen.ui as W

// Wallpaper thumbnail view.
//
// When `source` (a preview path or URL) is set, render it directly
// so animated formats (GIF/APNG/WebP) actually animate — the thumbnail
// pipeline transcodes to a single-frame PNG and would kill animation.
// When `source` is empty (typically video wallpapers), fall back to
// `W.ThumbnailRequest` which extracts a still frame from `resource`.
Item {
    id: root

    property string source
    property string resource
    property string wpType
    property int fillMode: Image.PreserveAspectFit
    property int radius: MD.Token.shape.corner.extra_small
    property bool useQExtra: true
    property bool retainWhileLoading: false
    property size sourceSize: Qt.size(-1, -1)
    property size maxSize: Qt.size(512, 512)
    property int verticalAlignment: Image.AlignVCenter

    readonly property bool _useDirect: root.source.length > 0
    readonly property url _displayUrl: _useDirect ? (/^[a-z][a-z0-9+.-]*:/i.test(root.source) ? root.source : Qt.url("file://" + root.source)) : req.cachePath

    readonly property int state: _useDirect ? W.ThumbnailRequest.Ready : req.state
    readonly property url cachePath: _displayUrl
    // The software scene graph runs neither QExtra's image node nor the
    // RoundClip shader effect; fall back to the stock image with square corners.
    readonly property bool _rhi: GraphicsInfo.api !== GraphicsInfo.Software
    readonly property bool _useQExtra: useQExtra && _rhi && /^(file|qrc|https?):/i.test(String(_displayUrl))

    readonly property real paintedWidth: m_image.item?.paintedWidth ?? 0
    readonly property real paintedHeight: m_image.item?.paintedHeight ?? 0
    readonly property int status: m_image.item?.status ?? Image.Null

    W.ThumbnailRequest {
        id: req
        source: ""
        resource: root._useDirect ? "" : root.resource
        wpType: root._useDirect ? "" : root.wpType
    }

    Loader {
        id: m_image
        anchors.fill: parent
        sourceComponent: root._useQExtra ? qextraImage : nativeImage
        layer.enabled: root._rhi
        layer.effect: MD.RoundClip {
            corners: MD.Util.corners(root.radius)
            size: Qt.vector2d(m_image.width, m_image.height)
        }
    }

    Component {
        id: qextraImage
        QE.AnimatedImage {
            sharedPlayback: true
            retainWhileLoading: root.retainWhileLoading
            source: root._displayUrl
            sourceSize: root.sourceSize
            maxSize: root.maxSize
            fillMode: root.fillMode
            verticalAlignment: root.verticalAlignment
            cache: true
            onSourceChanged: playing = true
        }
    }

    Component {
        id: nativeImage
        AnimatedImage {
            source: root._displayUrl
            sourceSize: root.sourceSize
            fillMode: root.fillMode
            verticalAlignment: root.verticalAlignment
            asynchronous: true
            cache: true
            playing: true
            onStatusChanged: if (status === AnimatedImage.Ready)
                playing = true
        }
    }
}
