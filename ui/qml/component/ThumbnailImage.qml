pragma ComponentBehavior: Bound
pragma ValueTypeBehavior: Assertable
import QtQuick
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
    property int    fillMode: Image.PreserveAspectFit
    property int    radius: MD.Token.shape.corner.extra_small

    readonly property bool _useDirect: root.source.length > 0
    readonly property url  _displayUrl: _useDirect
                                        ? (/^[a-z][a-z0-9+.-]*:/i.test(root.source)
                                           ? root.source
                                           : Qt.url("file://" + root.source))
                                        : req.cachePath

    // How large the preview is decoded. A card shows a few hundred pixels while
    // previews are routinely 1080 px wide, so bound the decode by the item's own
    // size: rounded up to a 64 px step, so a smooth resize does not re-decode on
    // every pixel, and doubled for the cropping fill mode, so a preview whose
    // aspect differs from the item still covers it.
    //
    // Animated sources keep their own size on purpose: QMovie scales the movie
    // to sourceSize instead of clamping it to the file, so a preview smaller
    // than the item would be decoded up and cost more than it saves.
    readonly property bool _animated: /\.(gif|apng|webp)(\?|#|$)/i.test(root.source)
    readonly property int  _decodeWidth: _animated ? -1
        : 64 * Math.max(1, Math.ceil(Math.max(root.width, root.height)
                                     * (root.fillMode === Image.PreserveAspectCrop ? 2 : 1) / 64))

    readonly property int    state    : _useDirect ? W.ThumbnailRequest.Ready : req.state
    readonly property url    cachePath: _displayUrl

    property alias paintedWidth     : m_image.paintedWidth
    property alias paintedHeight    : m_image.paintedHeight
    property alias status           : m_image.status
    property alias sourceSize       : m_image.sourceSize
    property alias verticalAlignment: m_image.verticalAlignment

    W.ThumbnailRequest {
        id: req
        source  : ""
        resource: root._useDirect ? "" : root.resource
        wpType  : root._useDirect ? "" : root.wpType
    }

    AnimatedImage {
        id: m_image
        anchors.fill: parent
        source: root._displayUrl
        fillMode: root.fillMode
        asynchronous: true
        cache: true
        sourceSize.width: root._decodeWidth
        playing: true
        // Loading a non-animated image flips `playing` to false; re-arm
        // it on every Ready so a later animated source resumes playback.
        onStatusChanged: if (status === AnimatedImage.Ready) playing = true
        layer.enabled: true
        layer.effect: MD.RoundClip {
            corners: MD.Util.corners(root.radius)
            size: Qt.vector2d(m_image.width, m_image.height)
        }
    }
}
