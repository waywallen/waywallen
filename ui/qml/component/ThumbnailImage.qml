pragma ComponentBehavior: Bound
pragma ValueTypeBehavior: Assertable
import QtQuick
import QExtra as QE
import Qcm.Material as MD
import waywallen.ui as W

// Preparation and final image decoding share one loading state.
Item {
    id: root

    readonly property url _displayUrl: req.cachePath
    readonly property bool _failed: state === W.ThumbnailRequest.Failed
    readonly property bool _hasInput: source.length > 0 || resource.length > 0
    // The software scene graph runs neither QExtra's image node nor the
    // RoundClip shader effect; fall back to the stock image with square corners.
    readonly property bool _rhi: GraphicsInfo.api !== GraphicsInfo.Software
    readonly property bool _useQExtra: useQExtra && _rhi
        && (String(_displayUrl).length === 0 || /^(file|qrc|https?):/i.test(String(_displayUrl)))
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
    readonly property bool hasFrame: _useQExtra ? (m_image.item?.hasFrame ?? false) : status === Image.Ready
    readonly property size intrinsicSize: root._useQExtra ? (m_image.item?.intrinsicSize ?? Qt.size(0, 0)) : Qt.size(0, 0)
    readonly property bool loading: state === W.ThumbnailRequest.Loading
    property int loadingMode: QE.Image.Automatic
    property size maxSize: Qt.size(512, 512)
    readonly property real paintedHeight: m_image.item?.paintedHeight ?? 0
    readonly property real paintedWidth: m_image.item?.paintedWidth ?? 0
    readonly property bool placeholderActive: placeholderVisible && loading && loadingMode === QE.Image.Automatic && visible
    readonly property bool placeholderVisible: shimmer !== null && _hasInput && !hasFrame && (loading || _failed)
    property Item preparationScope
    property int radius: MD.Token.shape.corner.extra_small
    property string resource
    property size resourceSize: Qt.size(0, 0)
    property bool retainWhileLoading: false
    property MD.Shimmer shimmer: W.ThumbnailLoading.shimmer
    property string source
    property size sourceSize: Qt.size(-1, -1)
    readonly property int state: {
        if (!_hasInput || req.state === W.ThumbnailRequest.Idle)
            return W.ThumbnailRequest.Idle;
        if (req.state === W.ThumbnailRequest.Loading)
            return W.ThumbnailRequest.Loading;
        if (req.state === W.ThumbnailRequest.Failed || status === (_useQExtra ? QE.Image.Error : Image.Error))
            return W.ThumbnailRequest.Failed;
        return status === (_useQExtra ? QE.Image.Ready : Image.Ready)
            ? W.ThumbnailRequest.Ready : W.ThumbnailRequest.Loading;
    }
    readonly property int status: m_image.item?.status ?? Image.Null
    property bool useQExtra: true
    property int verticalAlignment: Image.AlignVCenter
    property bool timeline: false
    property string wpType

    W.ThumbnailRequest {
        id: req

        resource: root.resource
        source: root.source
        timeline: root.timeline
        wpType: root.wpType
        enabled: root.visible && root.loadingMode !== QE.Image.Suspended
        prefetch: root.loadingMode === QE.Image.Prefetch
        retainWhileLoading: root.retainWhileLoading
        maximumEdge: Math.max(root.maxSize.width, root.maxSize.height) > 0
            ? Math.max(root.maxSize.width, root.maxSize.height) : 512
    }
    MD.Skeleton {
        anchors.fill: parent
        radius: root.radius
        shimmer: root.placeholderActive ? root.shimmer : null
        visible: root.placeholderVisible
    }
    Loader {
        id: m_image

        anchors.fill: parent
        layer.enabled: root._rhi && root.radius > 0
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
