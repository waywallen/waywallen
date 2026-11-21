module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/query/audio_output_query.moc"
#endif

export module waywallen:query.audio_output;
export import :query.query;

namespace waywallen
{

/// Fetches the audio output devices (sinks) the sound server currently
/// offers. The list is on demand: the daemon does not push changes, so the
/// settings page reloads it whenever it becomes visible.
export class AudioOutputListQuery : public Query,
                                    public QueryExtra<control::v1::Response, AudioOutputListQuery> {
    Q_OBJECT
    QML_ELEMENT

    /// List of `{ name, description, isDefault }` maps.
    Q_PROPERTY(QVariantList outputs READ outputs NOTIFY outputsChanged FINAL)
    /// False when the daemon could not reach a sound server.
    Q_PROPERTY(bool available READ available NOTIFY outputsChanged FINAL)

public:
    AudioOutputListQuery(QObject* parent = nullptr);

    auto outputs() const -> const QVariantList&;
    auto available() const -> bool;

    void reload() override;

    Q_SIGNAL void outputsChanged();

private:
    QVariantList m_outputs;
    bool         m_available { false };
};

} // namespace waywallen
