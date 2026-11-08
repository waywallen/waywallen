pragma Singleton
import QtQuick
import Qcm.Material as MD

QtObject {
    readonly property MD.Shimmer shimmer: MD.Shimmer {
        boundsMode: MD.Shimmer.Window
    }
}
