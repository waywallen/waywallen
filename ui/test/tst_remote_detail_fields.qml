pragma ComponentBehavior: Bound
import QtQuick
import QtTest
import "../qml/component" as W

TestCase {
    id: test
    name: "RemoteDetailFields"
    width: 480
    height: 320
    visible: true

    Component {
        id: fieldsComponent
        W.RemoteDetailFields {
            width: 440
        }
    }

    function test_emptyFields() {
        const fields = createTemporaryObject(fieldsComponent, test);
        verify(fields);
        compare(fields.visible, false);
        compare(fields.implicitHeight, 0);
    }

    function test_fieldsRemainOrderedAndPlainText() {
        const fields = createTemporaryObject(fieldsComponent, test, {
            fields: [
                {
                    key: "license",
                    label: "License",
                    value: "<b>CC BY</b>",
                    url: "https://creativecommons.org/licenses/by/4.0/"
                },
                {
                    key: "vertices",
                    label: "Vertices",
                    value: "0"
                }
            ],
            labelForField: field => "Translated " + field.label
        });
        verify(fields);
        const label = findChild(fields, "remoteFieldLabel-license");
        const license = findChild(fields, "remoteFieldValue-license");
        const vertices = findChild(fields, "remoteFieldValue-vertices");
        verify(label && license && vertices);
        compare(label.text, "Translated License");
        compare(license.text, "<b>CC BY</b>");
        compare(license.textFormat, Text.PlainText);
        compare(vertices.text, "0");
        tryVerify(() => license.parent.y < vertices.parent.y);
        compare(findChild(fields, "remoteFieldLink-license").visible, true);
        compare(findChild(fields, "remoteFieldLink-vertices").visible, false);

        fields.fields = [];
        compare(fields.visible, false);
        tryCompare(fields, "implicitHeight", 0);
    }
}
