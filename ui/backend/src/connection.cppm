module;
#include <cstddef>
#include <functional>
#include <future>
#include <memory>
#include <QByteArray>
#include <QObject>
#include <QString>
#include <QStringView>

export module waywallen.backend.connection;
export import :pending;
import rstd;

export namespace waywallen
{
struct BackendConnectionLimits {
    std::size_t frame_bytes { 64 * 1024 * 1024 };
    std::size_t message_bytes { 64 * 1024 * 1024 };
    std::size_t receive_messages { 64 };
    std::size_t receive_bytes { 64 * 1024 * 1024 };
    std::size_t send_messages { 256 };
    std::size_t send_bytes { 16 * 1024 * 1024 };
    std::size_t engine_bytes { 256 * 1024 * 1024 };
};

struct BackendConnectionCallbacks {
    std::function<void()>           connected;
    std::function<void(QByteArray)> message;
    std::function<void(QString)>    error;
    std::function<void()>           disconnected;
};

class BackendConnection : public QObject {
public:
    BackendConnection(rstd::async::RuntimeHandle, BackendConnectionCallbacks,
                      BackendConnectionLimits = {}, QObject* parent = nullptr);
    // shutdown() must finish before the owner event loop stops or this object is destroyed.
    ~BackendConnection();

    void open(QStringView url);
    auto send(QByteArray) -> rstd::Result<rstd::empty, QString>;
    void disconnect();
    auto shutdown() -> std::shared_future<void>;

private:
    class Private;
    std::unique_ptr<Private> d;
};
} // namespace waywallen
