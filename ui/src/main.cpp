#include <QGuiApplication>
#include <QCommandLineParser>
#include <QtQml/QQmlExtensionPlugin>
Q_IMPORT_QML_PLUGIN(waywallen_uiPlugin)

import ncrequest;
import waywallen;

int main(int argc, char** argv) {
    ncrequest::global_init();
    QGuiApplication gui_app(argc, argv);
    QCoreApplication::setApplicationName(APP_NAME);
    QCoreApplication::setApplicationVersion(APP_VERSION);

    QCommandLineParser parser;
    parser.addHelpOption();
    parser.addVersionOption();
    parser.addOption({"ws-port",
                      "Override the WebSocket port (normally discovered via DBus).",
                      "port"});
    parser.process(gui_app);

    quint16 ws_port = 0;
    if (parser.isSet("ws-port")) {
        bool ok = false;
        ws_port = parser.value("ws-port").toUShort(&ok);
        if (!ok) {
            qCritical("invalid --ws-port value: %s", qPrintable(parser.value("ws-port")));
            return 1;
        }
    }

    waywallen::App app(ws_port, {});
    app.init();

    return gui_app.exec();
}
