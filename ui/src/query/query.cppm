module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/query/query.moc"
#endif

export module waywallen:query.query;
export import qextra;
import :proto;

namespace waywallen
{

export template<typename Self>
using QueryExtra = QAsyncResultExtra<control::v1::Response, Self>;

export class Query : public QAsyncResult {
    Q_OBJECT

    Q_PROPERTY(bool delay READ delay WRITE setDelay NOTIFY delayChanged FINAL)
public:
    Query(QObject* parent = nullptr);
    ~Query();

    Q_SLOT void   delayReload();
    auto          delay() const -> bool;
    void          setDelay(bool v);
    Q_SIGNAL void delayChanged();

protected:
    template<typename T, typename R, typename... ARGS>
    void connect_requet_reload(R (T::*f)(ARGS...), T* obj) {
        connect(obj, f, this, &Query::delayReload);
    }
    template<typename T, typename R, typename... ARGS>
    void connect_requet_reload(R (T::*f)(ARGS...)) {
        connect(static_cast<T*>(this), f, this, &Query::delayReload);
    }

private:
    QTimer m_timer;
    bool   m_delay;
};

} // namespace waywallen
