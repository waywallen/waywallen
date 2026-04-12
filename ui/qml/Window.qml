pragma ValueTypeBehavior: Assertable
import QtCore
import QtQuick
import QtQml
import QtQuick.Window
import QtQuick.Templates as T

import Qcm.Material as MD

MD.ApplicationWindow {
    id: win
    MD.MProp.size.width: width
    MD.MProp.backgroundColor: {
        const c = MD.MProp.size.windowClass;
        switch (c) {
        case MD.Enum.WindowClassCompact:
            return MD.Token.color.surface;
        default:
            return MD.Token.color.surface_container;
        }
    }
    MD.MProp.textColor: MD.MProp.color.getOn(MD.MProp.backgroundColor)

    color: MD.MProp.backgroundColor
    height: 600
    visible: true
    width: 900
}
