pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.Popup {
    id: root

    property string sessionId: ""
    property string dismissedSessionId: ""
    property int loginState: 0
    property var titleSource: ""
    property var instructionSource: ""
    property var errorSource: ""
    readonly property bool busy: loginState >= 1 && loginState <= 3
    readonly property string errorText: W.I18n.tr(errorSource)
    readonly property string instruction: W.I18n.tr(instructionSource)
    readonly property string dialogTitle: W.I18n.tr(titleSource) || qsTranslate("QrLoginDialog", "Sign in")

    closePolicy: MD.Popup.CloseOnEscape
    dim: true
    modal: true
    parent: MD.Overlay.overlay
    x: Math.round((parent.width - width) / 2)
    y: Math.round((parent.height - height) / 2)
    bottomPadding: 24

    W.BrowserLoginCancelQuery {
        id: cancelQuery
        sessionId: root.sessionId
    }

    onClosed: {
        if (root.busy) {
            root.dismissedSessionId = root.sessionId;
            root.loginState = 6;
            cancelQuery.reload();
        }
    }

    Connections {
        target: W.Notify
        function onBrowserLoginProgress(sessionId, pluginId, actionId, state,
                                        displayValue, error, title, instruction) {
            if (state !== 1 && root.dismissedSessionId === sessionId)
                return;
            if (state !== 1 && root.sessionId.length > 0 && root.sessionId !== sessionId)
                return;
            if (state === 1) {
                root.sessionId = sessionId;
                root.dismissedSessionId = "";
                root.titleSource = "";
                root.instructionSource = "";
                root.errorSource = "";
            }
            root.loginState = state;
            if (W.I18n.tr(title).length > 0)
                root.titleSource = title;
            if (W.I18n.tr(instruction).length > 0)
                root.instructionSource = instruction;
            if (W.I18n.tr(error).length > 0)
                root.errorSource = error;
            if (root.busy || state === 5 || state === 7) {
                if (!root.visible)
                    root.open();
            } else if (state === 4) {
                const name = W.I18n.tr(displayValue);
                W.Action.toast(name.length > 0 ? qsTranslate("QrLoginDialog", "Signed in as %1").arg(name)
                    : qsTranslate("QrLoginDialog", "Signed in"));
                root.close();
            } else if (state === 6) {
                root.close();
            }
            if (state === 5 || state === 7)
                W.Global.toastError(root.errorText || (state === 7
                    ? qsTranslate("QrLoginDialog", "Sign-in expired")
                    : qsTranslate("QrLoginDialog", "Sign-in failed")));
        }
    }

    contentItem: ColumnLayout {
        spacing: 16

        MD.DialogHeader {
            Layout.fillWidth: true
            title: root.dialogTitle
        }

        MD.LinearIndicator {
            Layout.fillWidth: true
            Layout.leftMargin: 24
            Layout.rightMargin: 24
            visible: root.busy
        }

        MD.Label {
            Layout.fillWidth: true
            Layout.preferredWidth: 380
            Layout.maximumWidth: 480
            Layout.leftMargin: 24
            Layout.rightMargin: 24
            wrapMode: Text.WordWrap
            text: root.loginState === 1 ? qsTr("Opening the sign-in window…")
                : root.loginState === 3 ? qsTr("Verifying sign-in…")
                : root.busy ? (root.instruction || qsTr("Complete sign-in in the browser window."))
                : (root.errorText || (root.loginState === 7
                    ? qsTranslate("QrLoginDialog", "Sign-in expired")
                    : qsTranslate("QrLoginDialog", "Sign-in failed")))
            color: root.busy ? MD.Token.color.on_surface : MD.Token.color.error
        }

        MD.DialogButtonBox {
            Layout.fillWidth: true

            MD.Button {
                text: root.busy ? qsTranslate("QrLoginDialog", "Cancel") : qsTranslate("QrLoginDialog", "Close")
                mdState.type: MD.Enum.BtText
                MD.DialogButtonBox.buttonRole: MD.DialogButtonBox.RejectRole
                onClicked: root.close()
            }
        }
    }
}
