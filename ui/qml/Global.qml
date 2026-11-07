pragma Singleton
import QtCore
import QtQuick
import Qcm.Material as MD
import waywallen.ui as W
import QExtra as QE

// App-wide singleton state and derived theming.
QtObject {
    id: root

    property bool sidebarAutoExpand: true
    property bool singleUiEnabled: false
    property bool smoothScrolling: true
    property int networkCacheMaximumMiB: 1024
    readonly property var imageCacheOptions: [0, 64, 128, 256, 512]
    property int imageCacheMiB: 64
    readonly property var imageTextureCacheOptions: [0, 64, 128, 256]
    property int imageTextureCacheMiB: 0
    property string themeMode: "system"
    readonly property color defaultAccentColor: "#6750A4"
    property string accentMode: "system"
    property color accentColor: defaultAccentColor
    property string lastOpenedVersion: ""

    onNetworkCacheMaximumMiBChanged: W.App.setNetworkCacheMaximumSize(networkCacheMaximumMiB * 1024 * 1024)
    onThemeModeChanged: _applyThemeMode()
    onImageCacheMiBChanged: QE.ImageCache.capacityMiB = imageCacheMiB
    onAccentModeChanged: _applyAccentColor()
    onAccentColorChanged: {
        if (accentMode === "custom")
            _applyAccentColor();
    }

    Component.onCompleted: {
        W.App.setNetworkCacheMaximumSize(networkCacheMaximumMiB * 1024 * 1024);
        setThemeMode(themeMode);
        setAccentMode(accentMode);
        setImageCacheMiB(imageCacheMiB);
        setImageTextureCacheMiB(imageTextureCacheMiB);
        QE.ImageCache.capacityMiB = imageCacheMiB;
    }

    function setImageCacheMiB(value) {
        const requested = Number(value);
        let selected = 64;
        if (requested <= 0) {
            selected = 0;
        } else if (Number.isFinite(requested)) {
            for (let i = 1; i < imageCacheOptions.length; ++i) {
                const option = imageCacheOptions[i];
                if (Math.abs(option - requested) < Math.abs(selected - requested))
                    selected = option;
            }
        }
        imageCacheMiB = selected;
    }

    function setImageTextureCacheMiB(value) {
        const requested = Number(value);
        let selected = 0;
        if (Number.isFinite(requested) && requested > 0) {
            for (let i = 1; i < imageTextureCacheOptions.length; ++i) {
                const option = imageTextureCacheOptions[i];
                if (Math.abs(option - requested) < Math.abs(selected - requested))
                    selected = option;
            }
        }
        imageTextureCacheMiB = selected;
    }

    function setThemeMode(mode) {
        const normalized = mode === "light" || mode === "dark" ? mode : "system";
        if (themeMode !== normalized)
            themeMode = normalized;
        else
            _applyThemeMode();
    }

    function _applyThemeMode() {
        const system = themeMode === "system";
        MD.Token.color.useSysColorSM = system;
        if (!system)
            MD.Token.themeMode = themeMode === "dark" ? MD.Enum.Dark : MD.Enum.Light;
    }

    function setAccentMode(mode) {
        const normalized = mode === "custom" ? "custom" : "system";
        if (accentMode !== normalized)
            accentMode = normalized;
        else
            _applyAccentColor();
    }

    function _applyAccentColor() {
        const system = accentMode === "system";
        MD.Token.color.useSysAccentColor = system;
        if (!system)
            MD.Token.color.accentColor = accentColor;
    }

    function recordOpenedVersion(version) {
        if (version.length === 0)
            return false;
        const previous = lastOpenedVersion;
        if (previous !== version)
            lastOpenedVersion = version;
        return previous.length > 0 && previous !== version;
    }

    readonly property Component errorToastAction: Component {
        MD.Action {
            required property string error
            text: qsTr("Copy")
            onTriggered: {
                W.Action.copyToClipboard(error);
                W.Action.toast(qsTr("Copied to clipboard"), 2000);
            }
        }
    }

    function toastError(error) {
        const action = errorToastAction.createObject(root, {
            error: error
        });
        W.Action.toast(error, 0, MD.Enum.TFCloseable, action);
    }

    readonly property Settings _generalSettings: Settings {
        property alias sidebarAutoExpand: root.sidebarAutoExpand
        property alias singleUiEnabled: root.singleUiEnabled
        property alias smoothScrolling: root.smoothScrolling
        property alias networkCacheMaximumMiB: root.networkCacheMaximumMiB
        property alias imageCacheMiB: root.imageCacheMiB
        property alias imageTextureCacheMiB: root.imageTextureCacheMiB
        property alias themeMode: root.themeMode
        property alias accentMode: root.accentMode
        property alias accentColor: root.accentColor
        property alias lastOpenedVersion: root.lastOpenedVersion
    }

    // Per-vendor Material color schemes, seeded from each GPU vendor's brand
    // color and tracking the app theme mode, so vendor chips stay legible in
    // light and dark.
    readonly property QtObject gpu: QtObject {
        // PCI vendor IDs: AMD 0x1002, NVIDIA 0x10de, Intel 0x8086.
        readonly property MD.MdColorMgr amd: MD.MdColorMgr {
            accentColor: Qt.rgba(0.86, 0.20, 0.20, 1.0)
            mode: MD.Token.color.mode
            useSysColorSM: MD.Token.color.useSysColorSM
        }
        readonly property MD.MdColorMgr nvidia: MD.MdColorMgr {
            accentColor: Qt.rgba(0.27, 0.66, 0.20, 1.0)
            mode: MD.Token.color.mode
            useSysColorSM: MD.Token.color.useSysColorSM
        }
        readonly property MD.MdColorMgr intel: MD.MdColorMgr {
            accentColor: Qt.rgba(0.20, 0.45, 0.85, 1.0)
            mode: MD.Token.color.mode
            useSysColorSM: MD.Token.color.useSysColorSM
        }

        function forVendor(vendorId) {
            if (vendorId === 0x1002)
                return amd;
            if (vendorId === 0x10de)
                return nvidia;
            if (vendorId === 0x8086)
                return intel;
            return null;
        }
    }
}
