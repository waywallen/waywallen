pragma ComponentBehavior: Bound
import QtQml

QtObject {
    id: root

    property var canvasObject: null
    property var displays: []
    property var baseRevision: 0
    property string name: ""
    property var members: []
    property bool dirty: false
    property string baseline: ""

    function rowForMember(member) {
        const rect = member.rect || member || ({});
        const authored = member.authoredRect || rect;
        const key = String(member.settingsKey || "");
        const matches = (displays || []).filter(display => display.settingsKey === key);
        const width = Math.max(1, Number(rect.width || (matches[0]?.width ?? 1920)));
        const height = Math.max(1, Number(rect.height || (matches[0]?.height ?? 1080)));
        return {
            settingsKey: key,
            label: matches.length > 0 ? (matches[0].displayLabel || key) : key,
            x: Number(rect.x || 0),
            y: Number(rect.y || 0),
            width: width,
            height: height,
            authoredWidth: Number(authored.width || width),
            authoredHeight: Number(authored.height || height),
            sizeEdited: false,
            minimumScaleTo: member.minimumScaleTo || ({}),
            onlineCount: Number(member.onlineCount ?? matches.length),
            aspectLocked: member.aspectLocked ?? true
        };
    }

    function rowsForCanvas(canvas) {
        if (!canvas)
            return [];
        if (canvasObject?.id === canvas.id)
            return members;
        return (canvas.members || []).map(member => rowForMember(member));
    }

    function serializedState(stateName, stateMembers) {
        const persistedMembers = stateMembers.map(member => ({
                    settingsKey: String(member.settingsKey || ""),
                    x: Number(member.x || 0),
                    y: Number(member.y || 0),
                    width: Math.max(1, Number(member.width || 1)),
                    height: Math.max(1, Number(member.height || 1)),
                    aspectLocked: member.aspectLocked ?? true,
                    sizeEdited: member.sizeEdited ?? false
                }));
        return JSON.stringify({
            name: stateName,
            members: persistedMembers
        });
    }

    function serializedDraft() {
        return serializedState(name, members);
    }

    function refreshDirty() {
        dirty = serializedDraft() !== baseline;
    }

    function minimumSize(member) {
        const minimum = member.minimumScaleTo || ({});
        const width = Number(minimum.width || 0);
        const height = Number(minimum.height || 0);
        if (width <= 0 || height <= 0) {
            return {
                width: Math.max(1, Number(member.width || 1)),
                height: Math.max(1, Number(member.height || 1))
            };
        }
        return {
            width: width,
            height: height
        };
    }

    function sizeFromWidth(member, requestedWidth) {
        const minimum = minimumSize(member);
        const width = Math.max(minimum.width, Math.round(Number(requestedWidth || 0)));
        if (member.aspectLocked ?? true) {
            return {
                width: width,
                height: Math.max(1, Math.round(width * minimum.height / minimum.width))
            };
        }
        return {
            width: width,
            height: Math.max(minimum.height, Math.round(Number(member.height || 1)))
        };
    }

    function sizeFromHeight(member, requestedHeight) {
        const minimum = minimumSize(member);
        const height = Math.max(minimum.height, Math.round(Number(requestedHeight || 0)));
        if (member.aspectLocked ?? true) {
            return {
                width: Math.max(1, Math.round(height * minimum.width / minimum.height)),
                height: height
            };
        }
        return {
            width: Math.max(minimum.width, Math.round(Number(member.width || 1))),
            height: height
        };
    }

    function constrainedMember(member) {
        const size = (member.aspectLocked ?? true)
            ? sizeFromWidth(member, member.width)
            : {
                width: Math.max(minimumSize(member).width, Math.round(Number(member.width || 1))),
                height: Math.max(minimumSize(member).height, Math.round(Number(member.height || 1)))
            };
        return Object.assign({}, member, size);
    }

    function begin(canvas) {
        canvasObject = canvas;
        baseRevision = canvas?.revision || 0;
        name = canvas?.name || "";
        members = (canvas?.members || []).map(member => rowForMember(member));
        baseline = serializedDraft();
        dirty = false;
    }

    function refreshMemberConfig(canvas) {
        if (!canvas || canvasObject?.id !== canvas.id)
            return;
        canvasObject = canvas;
        baseRevision = canvas.revision || baseRevision;
        const currentRows = (canvas.members || []).map(member => rowForMember(member));
        const currentByKey = new Map(currentRows.map(member => [member.settingsKey, member]));
        const mergeDraft = rows => rows.map(member => {
                const current = currentByKey.get(member.settingsKey);
                if (!current)
                    return member;
                const updated = Object.assign({}, member, {
                    label: current.label,
                    minimumScaleTo: current.minimumScaleTo,
                    onlineCount: current.onlineCount,
                    authoredWidth: current.authoredWidth,
                    authoredHeight: current.authoredHeight
                });
                return member.sizeEdited ? constrainedMember(updated) : Object.assign(updated, {
                    width: current.width,
                    height: current.height
                });
            });
        const mergeBaseline = rows => rows.map(member => {
                const current = currentByKey.get(member.settingsKey);
                if (!current)
                    return member;
                return Object.assign({}, member, current);
            });
        members = mergeDraft(members);
        if (baseline.length) {
            const saved = JSON.parse(baseline);
            baseline = serializedState(saved.name, mergeBaseline(saved.members || []));
        }
        refreshDirty();
    }

    function clear() {
        canvasObject = null;
        baseRevision = 0;
        name = "";
        members = [];
        baseline = "";
        dirty = false;
    }

    function cancel() {
        if (canvasObject)
            begin(canvasObject);
    }

    function accept(revision) {
        if (revision !== undefined && Number(revision) > 0)
            baseRevision = Number(revision);
        members = members.map(member => Object.assign({}, member, {
            authoredWidth: member.sizeEdited ? member.width : member.authoredWidth,
            authoredHeight: member.sizeEdited ? member.height : member.authoredHeight,
            sizeEdited: false
        }));
        baseline = serializedDraft();
        dirty = false;
    }

    function setName(value) {
        if (name === value)
            return;
        name = value;
        refreshDirty();
    }

    function availableDisplays() {
        const keys = new Set(members.map(item => item.settingsKey));
        const rows = [];
        const seen = new Set();
        for (const display of displays || []) {
            const key = String(display.settingsKey || display.name || "");
            if (!key.length || keys.has(key) || seen.has(key))
                continue;
            if ((display.canvasId || "").length && display.canvasId !== canvasObject?.id)
                continue;
            seen.add(key);
            rows.push(display);
        }
        return rows;
    }

    function addDisplay(display) {
        const bounds = topologyBounds(members);
        addDisplayAt(display, members.length > 0 ? bounds.x + bounds.width + 80 : 0, 0);
    }

    function addDisplayAtEdge(display, pointerX) {
        if (members.length === 0) {
            addDisplayAt(display, 0, 0);
            return;
        }
        const bounds = topologyBounds(members);
        const width = Math.max(1, Number(display?.width || 1920));
        const placeLeft = Number(pointerX || 0) < bounds.x + bounds.width / 2;
        addDisplayAt(display, placeLeft ? bounds.x - width : bounds.x + bounds.width, bounds.y);
    }

    function addDisplayAt(display, x, y) {
        const key = String(display?.settingsKey || display?.name || "");
        if (!key.length || members.some(member => member.settingsKey === key))
            return;
        const next = members.slice();
        next.push({
            settingsKey: key,
            label: display.displayLabel,
            x: Math.round(Number(x || 0) / 10) * 10,
            y: Math.round(Number(y || 0) / 10) * 10,
            width: Math.max(1, Number(display.width || 1920)),
            height: Math.max(1, Number(display.height || 1080)),
            minimumScaleTo: {
                width: Math.max(1, Number(display.width || 1920)),
                height: Math.max(1, Number(display.height || 1080))
            },
            onlineCount: 1,
            aspectLocked: true
        });
        members = next;
        refreshDirty();
    }

    function removeAt(index) {
        if (index < 0 || index >= members.length)
            return;
        const next = members.slice();
        next.splice(index, 1);
        members = next;
        refreshDirty();
    }

    function replaceAt(index, values) {
        if (index < 0 || index >= members.length)
            return;
        const next = members.slice();
        next[index] = Object.assign({}, next[index], values);
        members = next;
        refreshDirty();
    }

    function setMemberWidth(index, value) {
        const member = members[index];
        if (!member)
            return;
        replaceAt(index, Object.assign(sizeFromWidth(member, value), { sizeEdited: true }));
    }

    function setMemberHeight(index, value) {
        const member = members[index];
        if (!member)
            return;
        replaceAt(index, Object.assign(sizeFromHeight(member, value), { sizeEdited: true }));
    }

    function setMemberAspectLocked(index, locked) {
        const member = members[index];
        if (!member)
            return;
        const next = Object.assign({}, member, {
            aspectLocked: locked
        });
        replaceAt(index, locked ? constrainedMember(next) : next);
    }

    function resetMemberSize(index) {
        const member = members[index];
        if (!member)
            return;
        const minimum = member.minimumScaleTo || ({});
        const width = Number(minimum.width || 0);
        const height = Number(minimum.height || 0);
        if (width <= 0 || height <= 0)
            return;
        replaceAt(index, {
            width: width,
            height: height,
            sizeEdited: true
        });
    }

    function snappedMemberPosition(index, x, y, threshold) {
        const member = members[index];
        if (!member)
            return {
                x: Number(x || 0),
                y: Number(y || 0),
                xSnapped: false,
                ySnapped: false
            };
        const width = Math.max(1, Number(member.width || 1));
        const height = Math.max(1, Number(member.height || 1));
        const limit = Math.max(0, Number(threshold || 0));
        let snappedX = Number(x || 0);
        let snappedY = Number(y || 0);
        let xDistance = limit + 1;
        let yDistance = limit + 1;
        for (let otherIndex = 0; otherIndex < members.length; ++otherIndex) {
            if (otherIndex === index)
                continue;
            const other = members[otherIndex];
            const left = Number(other.x || 0);
            const top = Number(other.y || 0);
            const right = left + Math.max(1, Number(other.width || 1));
            const bottom = top + Math.max(1, Number(other.height || 1));
            for (const candidate of [left, right, left - width, right - width]) {
                const distance = Math.abs(candidate - x);
                if (distance <= limit && distance < xDistance) {
                    snappedX = candidate;
                    xDistance = distance;
                }
            }
            for (const candidate of [top, bottom, top - height, bottom - height]) {
                const distance = Math.abs(candidate - y);
                if (distance <= limit && distance < yDistance) {
                    snappedY = candidate;
                    yDistance = distance;
                }
            }
        }
        return {
            x: snappedX,
            y: snappedY,
            xSnapped: xDistance <= limit,
            ySnapped: yDistance <= limit
        };
    }

    function moveTo(index, x, y, snapThreshold) {
        const position = snappedMemberPosition(index, x, y, snapThreshold);
        replaceAt(index, {
            x: position.xSnapped ? position.x : Math.round(Number(x || 0) / 10) * 10,
            y: position.ySnapped ? position.y : Math.round(Number(y || 0) / 10) * 10
        });
    }

    function moveBy(index, dx, dy) {
        const member = members[index];
        if (!member)
            return;
        moveTo(index, member.x + dx, member.y + dy, 0);
    }

    function topologyBounds(rows) {
        const values = rows || [];
        if (values.length === 0)
            return ({
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1
                });
        let left = Number(values[0].x || 0);
        let top = Number(values[0].y || 0);
        let right = left + Math.max(1, Number(values[0].width || 1));
        let bottom = top + Math.max(1, Number(values[0].height || 1));
        for (const item of values) {
            left = Math.min(left, Number(item.x || 0));
            top = Math.min(top, Number(item.y || 0));
            right = Math.max(right, Number(item.x || 0) + Math.max(1, Number(item.width || 1)));
            bottom = Math.max(bottom, Number(item.y || 0) + Math.max(1, Number(item.height || 1)));
        }
        return ({
                x: left,
                y: top,
                width: Math.max(1, right - left),
                height: Math.max(1, bottom - top)
            });
    }
}
