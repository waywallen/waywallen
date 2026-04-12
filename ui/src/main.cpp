#include <QGuiApplication>
#include <QtQml/QQmlExtensionPlugin>
Q_IMPORT_QML_PLUGIN(waywallen_uiPlugin)

import ncrequest;

int main(int argc, char **argv) {
  ncrequest::global_init();
  QGuiApplication gui_app(argc, argv);
}