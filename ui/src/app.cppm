module;
#ifdef Q_MOC_RUN
#    include "waywallen/app.moc"
#endif

#include "QExtra/macro_qt.hpp"

export module waywallen:app;
export import :backend;
export import :display;
export import :gpu;
export import :renderer;
export import :library;
export import :presentation;
import qextra;

class AppPrivate;

namespace waywallen
{
export class App : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_SINGLETON

    Q_PROPERTY(DisplayManager* displayManager READ displayManager CONSTANT FINAL)
    Q_PROPERTY(RendererManager* rendererManager READ rendererManager CONSTANT FINAL)
    Q_PROPERTY(LibraryManager* libraryManager READ libraryManager CONSTANT FINAL)
    Q_PROPERTY(GpuManager* gpuManager READ gpuManager CONSTANT FINAL)
    Q_PROPERTY(PresentationManager* presentationManager READ presentationManager CONSTANT FINAL)
    Q_PROPERTY(QString version READ version CONSTANT FINAL)
    Q_PROPERTY(qint64 networkCacheSize READ networkCacheSize NOTIFY networkCacheSizeChanged FINAL)
    Q_PROPERTY(qint64 networkCacheMaximumSize READ networkCacheMaximumSize NOTIFY
                   networkCacheMaximumSizeChanged FINAL)
    Q_PROPERTY(QString uiLanguage READ uiLanguage NOTIFY uiLanguageChanged FINAL)
    Q_PROPERTY(
        QString resolvedUiLanguage READ resolvedUiLanguage NOTIFY resolvedUiLanguageChanged FINAL)
    Q_PROPERTY(
        QVariantList availableUiLanguages READ availableUiLanguages NOTIFY uiLanguageChanged FINAL)

public:
    App(quint16 port, rstd::empty);
    virtual ~App();
    static App* create(QQmlEngine* qmlEngine, QJSEngine* jsEngine);

    // make qml prefer create
    App() = delete;

    void init();
    void registerUiRaiseService();
    void raiseMainWindow(const QString& xdgActivationToken = {});

    static auto instance() -> App*;

    auto displayManager() const -> DisplayManager*;
    auto rendererManager() const -> RendererManager*;
    auto libraryManager() const -> LibraryManager*;
    auto gpuManager() const -> GpuManager*;
    auto presentationManager() const -> PresentationManager*;
    auto version() const -> QString;
    auto networkCacheSize() const -> qint64;
    auto networkCacheMaximumSize() const -> qint64;
    auto uiLanguage() const -> const QString&;
    auto resolvedUiLanguage() const -> const QString&;
    auto availableUiLanguages() const -> QVariantList;

    auto engine() const -> QQmlApplicationEngine*;
    auto backend() const -> Backend*;

    Q_SLOT void      load_settings();
    Q_SLOT void      save_settings();
    Q_SLOT void      refreshNetworkCacheSize();
    Q_SLOT void      setNetworkCacheMaximumSize(qint64 size);
    Q_SLOT void      clearNetworkCache();
    Q_INVOKABLE bool setUiLanguage(const QString& language);
    Q_INVOKABLE void setSingleUiEnabled(bool enabled);

    Q_SIGNAL void errorOccurred(const QString& error);
    Q_SIGNAL void networkCacheSizeChanged();
    Q_SIGNAL void networkCacheMaximumSizeChanged();
    Q_SIGNAL void uiLanguageChanged();
    Q_SIGNAL void resolvedUiLanguageChanged();

protected:
    bool eventFilter(QObject* watched, QEvent* event) override;

private:
    QScopedPointer<AppPrivate> d_ptr;
    Q_DECLARE_PRIVATE(App);
};

/// Claim the UI well-known D-Bus name, or ask the existing instance to Raise.
/// Returns false when this process should exit (secondary instance).
export bool claimOrRaiseUiInstance();

class UiRaiseBus : public QObject {
    Q_OBJECT
    Q_CLASSINFO("D-Bus Interface", "org.waywallen.waywallen.UI1")
public:
    explicit UiRaiseBus(App* app): QObject(app), m_app(app) {}

    Q_SLOT void Raise(const QString& xdgActivationToken) {
        m_app->raiseMainWindow(xdgActivationToken);
    }

private:
    App* m_app;
};

} // namespace waywallen
