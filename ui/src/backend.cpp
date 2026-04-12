module;
#include "waywallen/backend.moc.h"

#undef assert
#include <rstd/macro.hpp>

module waywallen;
import :app;
import qextra;

using namespace Qt::Literals::StringLiterals;

#ifdef _WIN32
template<typename T>
using stream_type = asio::basic_stream_socket<asio::generic::stream_protocol, T>;
#else
template<typename T>
using stream_type = asio::posix::basic_stream_descriptor<T>;
#endif

namespace waywallen
{

namespace detail
{
class BackendHelper {
public:
    /*
        template<typename CompletionToken>
        static auto async_send(Backend& backend, msg::QcmMessage&& msg, CompletionToken&& token) {
            using ret = void(asio::error_code, msg::QcmMessage);
            return asio::async_initiate<CompletionToken, ret>(
                [&](auto&& handler) {
                    asio::dispatch(
                        backend.m_context->get_executor(),
                        [&backend, msg = std::move(msg), handler = std::move(handler)] mutable {
                            msg.setId_proto(backend.serial());
                            backend.m_handlers.insert_or_assign(
                                msg.id_proto(), cppstd::move_only_function<ret> { std::move(handler)
       }); auto bytes = msg.serialize(backend.m_serializer.get()); backend.m_client->send({
       bytes.constData(), (std::size_t)bytes.size() });
                        });
                },
                token);
        }
    */
};
} // namespace detail
/*
Backend::Backend(rc<ncrequest::Session> session)
    : m_thread(Box<QThread>::make()),
      m_context(
          Box<QtExecutionContext>::make(m_thread.get(), (QEvent::Type)QEvent::registerEventType())),
      m_process(new QProcess()),
      m_client(Box<ncrequest::WebSocketClient>::make(
          ncrequest::event::create<stream_type>(m_context->get_executor()), None(),
          mem_mgr().backend_mem)),
      m_serializer(Box<QProtobufSerializer>::make()),
      m_session(session),
      m_serial(1), // start from 1, as 0 is none
      m_port(0) {
    m_process->setWorkingDirectory(QCoreApplication::applicationDirPath());
    m_process->setProcessChannelMode(QProcess::ProcessChannelMode::ForwardedErrorChannel);
    m_client->set_on_error_callback([](auto err) {
        LOG_ERROR("{}", err);
    });
    m_client->set_on_connected_callback([this]() {
        Q_EMIT this->connected(m_port);
    });
    m_client->set_on_message_callback(
        [this, cache = make_rc<std::vector<byte>>()](std::span<const std::byte> bytes, bool last) {
            if (! last) {
                std::ranges::copy(bytes, std::back_inserter(*cache));
            } else {
                msg::QcmMessage msg;
                if (cache->empty()) {
                    msg.deserialize(m_serializer.get(), bytes);
                } else {
                    std::ranges::copy(bytes, std::back_inserter(*cache));
                    msg.deserialize(m_serializer.get(), *cache);
                    cache->clear();
                }
                if (msg.type() != msg::MessageTypeGadget::MessageType::PROVIDER_SYNC_STATUS_MSG) {
                    LOG_INFO("ws recv: {}", msg.type());
                }

                if (auto it = m_handlers.find(msg.id_proto()); it != m_handlers.end()) {
                    it->second(asio::error_code {}, std::move(msg));
                    m_handlers.erase(it);
                } else {
                    qcm::process_msg(std::move(msg));
                }
            }
        });
    // start thread
    {
        QObject::connect(m_thread.get(), &QThread::finished, m_process, &QObject::deleteLater);
        m_thread->start();
        bool ok = m_process->moveToThread(m_thread.get());
        debug_assert(ok);
    }
    // connect signal
    {
        connect(this, &Backend::started, this, &Backend::on_started);
        connect(this, &Backend::connected, this, &Backend::on_connected);
        connect(this, &Backend::error, this, &Backend::on_error);
        connect(m_process, &QProcess::finished, this, [this](int exitCode, QProcess::ExitStatus) {
            this->error(rstd::into(std::format("Backend exitd: {}", exitCode)));
        });
        connect(m_process, &QProcess::errorOccurred, this, [this](QProcess::ProcessError err) {
            this->error(rstd::into(std::format(
                "Backend {}", QMetaEnum::fromType<QProcess::ProcessError>().valueToKey((int)err))));
        });
    }
    {
        asio::post(m_context->get_executor(), [] {
            plt::set_thread_name("ws");
        });
    }
}

Backend::~Backend() {
    QMetaObject::invokeMethod(m_process, [self = m_process] {
        self->waitForFinished();
        LOG_WARN("backend stopped");
        self->thread()->quit();
    });
    m_client.reset();
    m_thread->wait();
}

auto Backend::start(QStringView exe_, QStringView data_dir_, QStringView cache_dir_) -> bool {
    m_exe       = exe_.toString();
    m_data_dir  = data_dir_.toString();
    m_cache_dir = cache_dir_.toString();

    {
        std::error_code ec;
        auto            path = cppstd::filesystem::path(m_exe.toStdString());

        if (! cppstd::filesystem::exists(path, ec)) {
            error(rstd::into(cppstd::format("Not found:\n {}", path.string())));
            return false;
        }
    }

    {
        struct State {
            QTextStream                           out { stdout, QIODevice::WriteOnly };
            rstd::Option<QMetaObject::Connection> cnn;
        };
        static auto state = rstd::rc::make_rc<State>();

        if (state->cnn) {
            QObject::disconnect(*(state->cnn));
        }

        state->cnn = Some(
            connect(m_process,
                    &QProcess::readyReadStandardOutput,
                    m_process,
                    [this, state = state] mutable {
                        m_process->setReadChannel(QProcess::ProcessChannel::StandardOutput);
                        if (m_process->canReadLine()) {
                            auto line = m_process->readLine();
                            auto doc  = QJsonDocument::fromJson(line);
                            if (auto jport = doc.object().value("port"); ! jport.isUndefined()) {
                                auto port = jport.toVariant().value<i32>();
                                LOG_INFO("backend port: {}", port);
                                Q_EMIT this->started(port);
                            } else {
                                this->error("Read port from backend failed");
                            }
                            QObject::disconnect(*(state->cnn));
                            state->cnn = Some(QObject::connect(
                                m_process,
                                &QProcess::readyReadStandardOutput,
                                m_process,
                                [p = m_process, state] mutable {
                                    p->setReadChannel(QProcess::ProcessChannel::StandardOutput);
                                    bool flush = p->canReadLine();
                                    state->out << p->readAllStandardOutput();
                                    if (flush) state->out.flush();
                                }));
                        }
                    }));
    }

    m_context->post([this, exe = m_exe, data_dir = m_data_dir, cache_dir = m_cache_dir] {
        LOG_DEBUG("starting backend: {}", exe);
        m_process->start(exe, { u"--data"_s, data_dir, u"--cache"_s, cache_dir });
    });
    return true;
}

void Backend::on_retry() { start(m_exe, m_data_dir, m_cache_dir); }

void Backend::on_error(QString) {
    m_context->post([this] {
        LOG_DEBUG("kill backend");
        m_process->kill();
    });
}

void Backend::on_started(i32 port) {
    m_port = port;
    m_client->connect(std::format("ws://127.0.0.1:{}", port));
}

void Backend::on_connected(i32) {
    auto msg = msg::QcmMessage();
    msg.setType(msg::MessageTypeGadget::MessageType::TEST_REQ);
    msg.setTestReq({});
    send_immediate(std::move(msg));
}

void Backend::send_immediate(msg::QcmMessage&& msg) {
    msg.setId_proto(serial());
    auto bytes = msg.serialize(m_serializer.get());
    m_client->send({ bytes.constData(), (std::size_t)bytes.size() });
}

auto Backend::send(msg::QcmMessage&& msg) -> task<Result<msg::QcmMessage, msg::Error>> {
    auto [ec, var] =
        co_await detail::BackendHelper::async_send(*this, std::move(msg), qcm::as_tuple(use_task));
    co_return Ok(var);
}

auto Backend::base() const -> std::string {
    return rstd::into(rstd::format("http://127.0.0.1:{}", m_port));
}

auto Backend::image(QStringView item_type, QStringView id, QStringView image_type)
    -> ncrequest::Request {
    auto url = rstd::format("{}/image/{}/{}/{}", this->base(), item_type, id, image_type);
    return ncrequest::Request { std::string_view(url) };
}
auto Backend::image(model::ItemId id, enums::ImageType image_type) -> ncrequest::Request {
    auto type = id.type();
    if (type == enums::ItemType::ItemAlbumArtist) type = enums::ItemType::ItemArtist;
    auto url = rstd::format("{}/image/{}/{}/{}", this->base(), type, id.id(), image_type);
    return ncrequest::Request { std::string_view(url) };
}

auto Backend::audio_url(model::ItemId id) -> QUrl {
    return rstd::into(rstd::format("{}/audio/{}/{}", this->base(), id.type(), id.id()));
}

auto Backend::serial() -> i32 {
    i32 cur = m_serial.load();
    for (;;) {
        const i32 to = (cur + 1) % std::numeric_limits<i32>::max();
        if (m_serial.compare_exchange_strong(cur, to)) {
            break;
        }
    }
    return cur;
}
    */

} // namespace waywallen

#include "waywallen/backend.moc"
