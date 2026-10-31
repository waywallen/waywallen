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

    readonly property url _displayUrl: _useDirect ? (/^[a-z][a-z0-9+.-]*:/i.test(root.source) ? root.source : Qt.url("file://" + root.source)) : req.cachePath
    readonly property bool _useDirect: root.source.length > 0
    readonly property bool _useQExtra: useQExtra && /^(file|qrc|https?):/i.test(String(_displayUrl))
    readonly property real aspectRatio: {
        if (resourceSize.width > 0 && resourceSize.height > 0)
            return resourceSize.width / resourceSize.height;
        if (intrinsicSize.width > 0 && intrinsicSize.height > 0)
            return intrinsicSize.width / intrinsicSize.height;
        return Math.max(0.1, fallbackAspectRatio);
    }
    readonly property url cachePath: _displayUrl
    property real fallbackAspectRatio: 1
    property int fillMode: Image.PreserveAspectFit
    readonly property size intrinsicSize: root._useQExtra ? (m_image.item?.intrinsicSize ?? Qt.size(0, 0)) : Qt.size(0, 0)
    property int loadingMode: QE.Image.Automatic
    property size maxSize: Qt.size(512, 512)
    readonly property real paintedHeight: m_image.item?.paintedHeight ?? 0
    readonly property real paintedWidth: m_image.item?.paintedWidth ?? 0
    property Item preparationScope
    property int radius: MD.Token.shape.corner.extra_small
    property string resource
    property size resourceSize: Qt.size(0, 0)
    property bool retainWhileLoading: false
    property string source
    property size sourceSize: Qt.size(-1, -1)
    readonly property int state: _useDirect ? W.ThumbnailRequest.Ready : req.state
    readonly property int status: m_image.item?.status ?? Image.Null
    property bool useQExtra: true
    property int verticalAlignment: Image.AlignVCenter
    property string wpType

    W.ThumbnailRequest {
        id: req

        resource: root._useDirect ? "" : root.resource
        source: ""
        wpType: root._useDirect ? "" : root.wpType
    }
    Loader {
        id: m_image

        anchors.fill: parent
        layer.enabled: root.radius > 0
        sourceComponent: root._useQExtra ? qextraImage : nativeImage

        layer.effect: MD.RoundClip {
            corners: MD.Util.corners(root.radius)
            size: Qt.vector2d(m_image.width, m_image.height)
        }
    }
    Component {
        id: qextraImage

        QE.AnimatedImage {
            cache: true
            fillMode: root.fillMode
            loadingMode: root.loadingMode
            maxSize: root.maxSize
            preparationScope: root.preparationScope
            retainWhileLoading: root.retainWhileLoading
            sharedPlayback: true
            source: root._displayUrl
            sourceSize: root.sourceSize
            verticalAlignment: root.verticalAlignment

            onSourceChanged: playing = true
        }
    }
    Component {
        id: nativeImage

        AnimatedImage {
            asynchronous: true
            cache: true
            fillMode: root.fillMode
            playing: root.loadingMode === QE.Image.Automatic
            source: root._displayUrl
            sourceSize: root.sourceSize
            verticalAlignment: root.verticalAlignment

            onStatusChanged: if (status === AnimatedImage.Ready)
                playing = Qt.binding(() => root.loadingMode === QE.Image.Automatic)
        }
    }
}
