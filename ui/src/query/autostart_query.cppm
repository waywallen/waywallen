module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/query/autostart_query.moc"
#endif

export module waywallen:query.autostart;
export import :query.query;

namespace waywallen
{

export class AutostartGetQuery : public Query,
                                 public QueryExtra<control::v1::Response, AutostartGetQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(bool enabled READ enabled NOTIFY enabledChanged FINAL)
    Q_PROPERTY(bool supported READ supported NOTIFY availabilityChanged FINAL)
    Q_PROPERTY(QString unavailableReason READ unavailableReason NOTIFY availabilityChanged FINAL)

public:
    AutostartGetQuery(QObject* parent = nullptr);

    bool    enabled() const;
    bool    supported() const { return m_supported; }
    QString unavailableReason() const { return m_unavailable_reason; }
    void    reload() override;

    Q_SIGNAL void enabledChanged();
    Q_SIGNAL void availabilityChanged();

private:
    bool    m_enabled   = false;
    bool    m_supported = false;
    QString m_unavailable_reason;
};

export class AutostartSetQuery : public Query,
                                 public QueryExtra<control::v1::Response, AutostartSetQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(bool enabled READ enabled WRITE setEnabled NOTIFY enabledChanged FINAL)

public:
    AutostartSetQuery(QObject* parent = nullptr);

    bool enabled() const;
    void setEnabled(bool enabled);
    void reload() override;

    Q_SIGNAL void enabledChanged();

private:
    bool m_enabled = false;
};

} // namespace waywallen
