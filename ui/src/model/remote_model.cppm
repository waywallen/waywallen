module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/model/remote_model.moc"
#endif

export module waywallen:model.remote;
export import :model.share_store;
import qextra;

export namespace waywallen::model
{

struct RemoteRow {
    Q_GADGET
    Q_PROPERTY(QString sourceId MEMBER sourceId)
    Q_PROPERTY(QString itemId MEMBER itemId)
    Q_PROPERTY(QString title MEMBER title)
    Q_PROPERTY(QString previewUrl MEMBER previewUrl)
    Q_PROPERTY(QString author MEMBER author)
    Q_PROPERTY(QString authorAvatarUrl MEMBER authorAvatarUrl)
    Q_PROPERTY(QString wpType MEMBER wpType)
    Q_PROPERTY(int acquisitionState MEMBER acquisitionState)

public:
    QString sourceId;
    QString itemId;
    QString title;
    QString previewUrl;
    QString author;
    QString authorAvatarUrl;
    QString wpType;
    int     acquisitionState { 0 };
    bool    operator==(const RemoteRow&) const = default;
};

inline auto remoteKey(const QString& sourceId, const QString& itemId) -> QString {
    QString key;
    key.reserve(sourceId.size() + itemId.size() + 1);
    key.append(sourceId);
    key.append(QChar::Null);
    key.append(itemId);
    return key;
}

} // namespace waywallen::model

template<>
struct kstore::ItemTrait<waywallen::model::RemoteRow> {
    using Self       = waywallen::model::RemoteRow;
    using key_type   = QString;
    using store_type = waywallen::ShareStore<Self>;

    static auto key(const Self& item) -> QString {
        return waywallen::model::remoteKey(item.sourceId, item.itemId);
    }
};

export namespace waywallen::model
{

class RemoteListModel
    : public kstore::QGadgetListModel,
      public kstore::QMetaListModelCRTP<RemoteRow, RemoteListModel, kstore::ListStoreType::Share> {
    Q_OBJECT
    QML_ANONYMOUS

    Q_PROPERTY(int count READ count NOTIFY countChanged FINAL)

    using list_crtp_t =
        kstore::QMetaListModelCRTP<RemoteRow, RemoteListModel, kstore::ListStoreType::Share>;

public:
    explicit RemoteListModel(QObject* parent = nullptr);

    auto count() const -> int { return static_cast<int>(size()); }

    void                    reset(QList<RemoteRow> rows, bool hasMore);
    void                    append(const QList<RemoteRow>& rows, bool hasMore);
    Q_INVOKABLE QStringList itemIds() const;

    Q_SIGNAL void countChanged();
};

} // namespace waywallen::model
