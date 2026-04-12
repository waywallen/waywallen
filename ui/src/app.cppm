module;
#ifdef Q_MOC_RUN
#    include "waywallen/app.moc"
#endif

#include "QExtra/macro_qt.hpp"

export module waywallen:app;
export import :backend;
export import qextra;

class AppPrivate;

namespace waywallen
{
export class App : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_SINGLETON

public:
    App(quint16 port, rstd::empty);
    virtual ~App();
    static App* create(QQmlEngine* qmlEngine, QJSEngine* jsEngine);

    // make qml prefer create
    App() = delete;

    void init();

    static auto instance() -> App*;
    auto        engine() const -> QQmlApplicationEngine*;
    auto        backend() const -> Backend*;

    Q_SLOT void load_settings();
    Q_SLOT void save_settings();

private:
    QScopedPointer<AppPrivate> d_ptr;
    Q_DECLARE_PRIVATE(App);
};
} // namespace waywallen
