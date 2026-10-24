module;

#include <QtQml/QQmlExtensionPlugin>

Q_IMPORT_QML_PLUGIN(waywallen_uiPlugin)
Q_IMPORT_QML_PLUGIN(waywallen_controlPlugin)
Q_IMPORT_QML_PLUGIN(QExtraPlugin)

module waywallen.entry;
import qextra;

import rstd.cppstd;
import waywallen;

namespace waywallen
{
int run(int argc, char** argv) {
    QGuiApplication gui_app(argc, argv);
    gui_app.setDesktopFileName(APP_ID);
    gui_app.setOrganizationName("waywallen");
    gui_app.setOrganizationDomain("waywallen.org");
    gui_app.setApplicationName(APP_NAME);
    gui_app.setApplicationVersion(LITO_PKG_VERSION);

    QCommandLineParser parser;
    parser.addHelpOption();
    parser.addVersionOption();
    parser.addOption(
        { "ws-port", "Override the WebSocket port (normally discovered via DBus).", "port" });
    parser.process(gui_app);

    quint16 ws_port = 0;
    if (parser.isSet("ws-port")) {
        bool ok = false;
        ws_port = parser.value("ws-port").toUShort(&ok);
        if (! ok) {
            qCritical("invalid --ws-port value: %s", qPrintable(parser.value("ws-port")));
            return 1;
        }
    }

    QSettings  settings;
    const bool single_ui = settings.value(QStringLiteral("singleUiEnabled"), false).toBool();
    if (single_ui && ! claimOrRaiseUiInstance()) {
        return 0;
    }

    App app(ws_port, {});
    app.init();
    if (single_ui) {
        app.registerUiRaiseService();
    }

    return gui_app.exec();
}
} // namespace waywallen
