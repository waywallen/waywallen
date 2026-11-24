module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/action.moc"
#endif

export module waywallen:action;
import qextra;

namespace waywallen
{

export class Action : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_SINGLETON
    Q_PROPERTY(QObject* wallpaperSelectStorage READ wallpaperSelectStorage NOTIFY
                   wallpaperSelectStorageChanged FINAL)
public:
    Action(QObject* parent);
    ~Action() override;
    Action() = delete;

    static auto    instance() -> Action*;
    static Action* create(QQmlEngine*, QJSEngine*);

    auto wallpaperSelectStorage() const -> QObject*;

    Q_INVOKABLE void enterWallpaperSelect(QObject* storage);
    Q_INVOKABLE void copyToClipboard(const QString& text);
    /// True when the waywallen-display KDE wallpaper plugin is installed.
    Q_INVOKABLE bool hasPlasmaWallpaperPlugin() const;
    /// Ask plasmashell to use the Waywallen wallpaper plugin on every
    /// desktop. Reports back through `plasmaWallpaperPluginSet`.
    Q_INVOKABLE void usePlasmaWallpaperPlugin();

Q_SIGNALS:
    void toast(QString text, qint32 duration = 3000, qint32 flags = 0, QObject* action = nullptr);
    void wallpaperSelectStorageChanged();
    void wallpaperSelectEntered(QObject* storage);
    /// `error` is empty on success.
    void plasmaWallpaperPluginSet(QString error);

private:
    QPointer<QObject> m_wallpaper_select_storage;
};

} // namespace waywallen
