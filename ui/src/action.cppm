module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/action.moc"
#endif

export module waywallen:action;
export import qextra;

namespace waywallen
{

export class Action : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_SINGLETON
public:
    Action(QObject* parent);
    ~Action() override;
    Action() = delete;

    static auto    instance() -> Action*;
    static Action* create(QQmlEngine*, QJSEngine*);

Q_SIGNALS:
    void toast(QString text, qint32 duration = 3000, qint32 flags = 0,
               QObject* action = nullptr);
};

} // namespace waywallen
