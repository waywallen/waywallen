import QtQuick
import QtTest
import "../qml/page/display" as Display

TestCase {
    name: "CanvasEditor"

    Display.CanvasEditorState {
        id: editor
    }

    function canvas(width, height, authoredWidth = 1920, authoredHeight = 1080, aspectLocked = true, online = true) {
        return {
            id: "canvas",
            name: "Desktop",
            revision: 1,
            members: [
                {
                    settingsKey: "display",
                    rect: {
                        x: 0,
                        y: 0,
                        width: width,
                        height: height
                    },
                    authoredRect: {
                        x: 0,
                        y: 0,
                        width: authoredWidth,
                        height: authoredHeight
                    },
                    minimumScaleTo: online ? {
                        width: width,
                        height: height
                    } : {},
                    aspectLocked: aspectLocked,
                    onlineCount: online ? 1 : 0
                }
            ]
        };
    }

    function init() {
        editor.begin(canvas(1920, 1080));
        verify(!editor.dirty);
    }

    function test_unedited_size_recovers_after_transient_resize() {
        editor.refreshMemberConfig(canvas(3840, 2160));
        compare(editor.members[0].width, 3840);
        compare(editor.draftMembers()[0].width, 1920);
        compare(editor.draftMembers()[0].height, 1080);
        verify(!editor.dirty);
        editor.refreshMemberConfig(canvas(1920, 1080));
        compare(editor.members[0].width, 1920);
        verify(!editor.dirty);
    }

    function test_reverting_to_original_size_is_clean_data() {
        return [
            {
                tag: "width-online",
                axis: "width",
                online: true
            },
            {
                tag: "height-online",
                axis: "height",
                online: true
            },
            {
                tag: "width-offline",
                axis: "width",
                online: false
            },
            {
                tag: "height-offline",
                axis: "height",
                online: false
            }
        ];
    }

    function test_reverting_to_original_size_is_clean(data) {
        editor.begin(canvas(1920, 1080, 1920, 1080, true, data.online));
        if (data.axis === "width")
            editor.setMemberWidth(0, 2560);
        else
            editor.setMemberHeight(0, 1440);
        verify(editor.dirty);
        if (data.axis === "width")
            editor.setMemberWidth(0, 1920);
        else
            editor.setMemberHeight(0, 1080);
        compare(editor.members[0].width, 1920);
        compare(editor.members[0].height, 1080);
        verify(!editor.dirty);
    }

    function test_pending_size_survives_transient_resize_data() {
        return [
            {
                tag: "width-locked",
                axis: "width",
                locked: true,
                width: 2560,
                height: 1440
            },
            {
                tag: "height-locked",
                axis: "height",
                locked: true,
                width: 2560,
                height: 1440
            },
            {
                tag: "width-unlocked",
                axis: "width",
                locked: false,
                width: 2560,
                height: 1080
            },
            {
                tag: "height-unlocked",
                axis: "height",
                locked: false,
                width: 1920,
                height: 1440
            }
        ];
    }

    function test_pending_size_survives_transient_resize(data) {
        editor.begin(canvas(1920, 1080, 1920, 1080, data.locked));
        if (data.axis === "width")
            editor.setMemberWidth(0, data.width);
        else
            editor.setMemberHeight(0, data.height);
        editor.refreshMemberConfig(canvas(3840, 2160, 1920, 1080, data.locked));
        compare(editor.members[0].width, 3840);
        compare(editor.members[0].height, 2160);
        compare(editor.draftMembers()[0].width, data.width);
        compare(editor.draftMembers()[0].height, data.height);
        verify(editor.dirty);
        editor.refreshMemberConfig(canvas(1920, 1080, 1920, 1080, data.locked));
        compare(editor.members[0].width, data.width);
        compare(editor.members[0].height, data.height);
        compare(editor.draftMembers()[0].width, data.width);
        compare(editor.draftMembers()[0].height, data.height);
    }

    function test_moving_and_renaming_do_not_save_runtime_size() {
        editor.begin(canvas(3840, 2160));
        editor.moveBy(0, 100, 20);
        verify(editor.dirty);
        compare(editor.members[0].width, 3840);
        compare(editor.draftMembers()[0].width, 1920);
        compare(editor.draftMembers()[0].height, 1080);
        editor.moveBy(0, -100, -20);
        verify(!editor.dirty);
        editor.setName("Other desktop");
        verify(editor.dirty);
        compare(editor.draftMembers()[0].width, 1920);
        editor.setName("Desktop");
        verify(!editor.dirty);
    }

    function test_aspect_lock_only_changes_effective_size() {
        const snapshot = canvas(1920, 1200, 1920, 1200, false);
        snapshot.members[0].minimumScaleTo = {
            width: 1920,
            height: 1080
        };
        editor.begin(snapshot);
        editor.setMemberAspectLocked(0, true);
        compare(editor.members[0].height, 1080);
        compare(editor.draftMembers()[0].height, 1200);
        verify(editor.dirty);
        editor.setMemberAspectLocked(0, false);
        compare(editor.members[0].height, 1200);
        verify(!editor.dirty);
    }

    function test_reset_can_undo_a_size_change() {
        editor.setMemberWidth(0, 2560);
        editor.resetMemberSize(0);
        compare(editor.draftMembers()[0].width, 1920);
        compare(editor.draftMembers()[0].height, 1080);
        verify(!editor.dirty);
    }

    function test_accept_keeps_requested_size_during_runtime_growth() {
        editor.setMemberWidth(0, 2560);
        editor.refreshMemberConfig(canvas(3840, 2160));
        editor.accept(2);
        compare(editor.baseRevision, 2);
        compare(editor.members[0].width, 3840);
        compare(editor.members[0].authoredWidth, 2560);
        compare(editor.draftMembers()[0].width, 2560);
        verify(!editor.dirty);
        const snapshot = canvas(2560, 1440, 2560, 1440);
        snapshot.revision = 2;
        snapshot.members[0].minimumScaleTo = {
            width: 1920,
            height: 1080
        };
        editor.refreshMemberConfig(snapshot);
        compare(editor.members[0].width, 2560);
        verify(!editor.dirty);
    }

    function test_accept_uses_an_already_received_canonical_snapshot() {
        editor.setMemberWidth(0, 2560);
        const snapshot = canvas(3840, 2160, 3840, 2160);
        snapshot.revision = 2;
        editor.refreshMemberConfig(snapshot);
        compare(editor.draftMembers()[0].width, 2560);
        verify(editor.dirty);
        editor.accept(2);
        compare(editor.draftMembers()[0].width, 3840);
        compare(editor.draftMembers()[0].height, 2160);
        compare(editor.members[0].authoredWidth, 3840);
        verify(!editor.dirty);
    }

    function test_canonical_size_updates_preserve_position_drafts() {
        editor.moveBy(0, 100, 20);
        editor.refreshMemberConfig(canvas(2560, 1440, 2560, 1440));
        compare(editor.draftMembers()[0].width, 2560);
        compare(editor.draftMembers()[0].x, 100);
        verify(editor.dirty);
        editor.moveBy(0, -100, -20);
        verify(!editor.dirty);
    }

    function test_new_members_produce_configuration_inputs() {
        const snapshot = canvas(1920, 1080);
        snapshot.members = [];
        editor.begin(snapshot);
        editor.addDisplayAt({
            settingsKey: "new",
            width: 1280,
            height: 720
        }, 100, 20);
        const inputs = editor.draftMembers();
        compare(inputs.length, 1);
        compare(inputs[0].settingsKey, "new");
        compare(inputs[0].width, 1280);
        compare(inputs[0].height, 720);
        compare(inputs[0].x, 100);
        compare(inputs[0].y, 20);
        compare(Object.keys(inputs[0]).sort().join(","), "aspectLocked,height,settingsKey,width,x,y");
        verify(editor.dirty);
        editor.removeAt(0);
        verify(!editor.dirty);
    }
}
