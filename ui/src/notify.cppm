module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/notify.moc"
#endif

export module waywallen:notify;
export import qextra;

namespace waywallen
{

export class Notify : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_SINGLETON

public:
    enum class Severity {
        Info,
        Success,
        Warning,
        Error,
    };
    Q_ENUM(Severity)

    Notify(QObject* parent);
    ~Notify() override;
    // QML should always reach us through `create` so we stay a
    // singleton parented to App.
    Notify() = delete;

    static auto    instance() -> Notify*;
    static Notify* create(QQmlEngine*, QJSEngine*);

    Q_INVOKABLE void info(const QString& message);
    Q_INVOKABLE void success(const QString& message);
    Q_INVOKABLE void warning(const QString& message);
    Q_INVOKABLE void error(const QString& message);
    Q_INVOKABLE void post(Severity sev, const QString& message);

Q_SIGNALS:
    void notified(Severity sev, const QString& message);
};

} // namespace waywallen
