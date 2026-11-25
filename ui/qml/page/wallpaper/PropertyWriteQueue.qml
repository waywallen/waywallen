pragma ComponentBehavior: Bound
import QtQml

QtObject {
    id: root

    readonly property bool busy: current !== null || entries.length > 0
    property var current: null
    property var entries: []

    signal writeRequested(string wallpaperId, string key, string value, bool reset)
    signal writeFailed(string wallpaperId, string error)
    signal drained()

    function enqueue(wallpaperId, key, value, reset) {
        const pending = entries.slice();
        const index = pending.findIndex(e => e.wallpaperId === wallpaperId && e.key === key);
        const edit = { wallpaperId, key, value, reset };
        if (index < 0)
            pending.push(edit);
        else
            pending[index] = edit;
        entries = pending;
        debounce.restart();
    }

    function flush() {
        debounce.stop();
        drain();
    }

    function drain() {
        if (current !== null || debounce.running)
            return;
        if (entries.length === 0) {
            drained();
            return;
        }
        const edit = entries[0];
        current = edit;
        entries = entries.slice(1);
        writeRequested(edit.wallpaperId, edit.key, edit.value, edit.reset);
    }

    function acknowledge(ok, error) {
        if (current === null)
            return;
        if (!ok) {
            const id = current.wallpaperId;
            entries = entries.filter(e => e.wallpaperId !== id);
            if (entries.length === 0)
                debounce.stop();
            writeFailed(id, error);
        }
        current = null;
        Qt.callLater(root.drain);
    }

    readonly property Timer debounce: Timer {
        interval: 200
        onTriggered: root.drain()
    }
}
