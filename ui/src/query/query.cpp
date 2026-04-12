module;
#include "waywallen/query/query.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import :query.query;
import qextra;

namespace waywallen
{

Query::Query(QObject* parent): QAsyncResult(parent), m_delay(true) {
    setForwardError(true);
    m_timer.setSingleShot(true);
    connect(&m_timer, &QTimer::timeout, this, &Query::reload);
}

Query::~Query() {}

void Query::delayReload() {
    if (delay()) {
        m_timer.setInterval(100);
        m_timer.start();
    } else {
        reload();
    }
}

auto Query::delay() const -> bool { return m_delay; }
void Query::setDelay(bool v) {
    if (m_delay != v) {
        m_delay = v;
        Q_EMIT delayChanged();
    }
}

} // namespace waywallen

#include "waywallen/query/query.moc.cpp"
