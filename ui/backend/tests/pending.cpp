#include <QByteArray>
#include <QString>
#include <cstdio>
#include <exception>
#include <stdexcept>

import waywallen.backend.connection;
import rstd;

namespace
{
void require(bool condition, const char* message) {
    if (! condition) throw std::runtime_error(message);
}
} // namespace

int main() {
    try {
        auto runtime = rstd::async::RuntimeBuilder::current_thread().enable_all().build().unwrap();
        std::exception_ptr failure;
        auto               run = [&]() -> rstd::async::coro<void> {
            try {
                using Completion = rstd::async::Completion<QByteArray, QString>;
                waywallen::BackendPending<QByteArray, QString> pending;
                auto                                           first  = Completion::make().unwrap();
                auto                                           second = Completion::make().unwrap();
                require(pending.add(1, rstd::move(first.get<1>())), "first request rejected");
                require(pending.add(2, rstd::move(second.get<1>())), "second request rejected");
                require(pending.contains(1), "request was not registered");
                auto canceler = pending.clone();
                canceler.cancel(1);
                auto canceled = co_await rstd::move(first.get<0>());
                require(canceled.is_err() && canceled.unwrap_err().is_canceled(),
                        "business cancellation did not release its response");
                require(! pending.complete(1, "late"), "canceled response was delivered");
                require(pending.complete(2, "second"), "other request was canceled");
                require(! pending.complete(2, "duplicate"), "response completed twice");
                auto received = co_await rstd::move(second.get<0>());
                require(received.is_ok() && received.unwrap() == "second", "wrong response");

                auto dropped = Completion::make().unwrap();
                require(pending.add(3, rstd::move(dropped.get<1>())), "dropped request rejected");
                dropped.get<0>().close();
                require(pending.complete(3, "already canceled"), "closed receiver was not removed");
                require(! pending.contains(3), "closed receiver stayed registered");

                auto failed    = Completion::make().unwrap();
                auto duplicate = Completion::make().unwrap();
                require(pending.add(4, rstd::move(failed.get<1>())), "failed request rejected");
                require(! pending.add(4, rstd::move(duplicate.get<1>())), "duplicate id accepted");
                auto rejected = co_await rstd::move(duplicate.get<0>());
                require(rejected.is_err() && rejected.unwrap_err().is_canceled(),
                        "duplicate replaced the original response");
                pending.fail_all(QStringLiteral("disconnected"));
                auto error = co_await rstd::move(failed.get<0>());
                require(error.is_err(), "pending was not failed");
                auto reason = rstd::move(error).unwrap_err();
                require(reason.is_failed(), "pending failure was reported as cancellation");
                require(rstd::move(reason).unwrap_failed() == QStringLiteral("disconnected"),
                        "disconnect reason was lost");
                require(! pending.contains(4) && ! pending.complete(4, "late"),
                        "failed response stayed registered");

                constexpr std::uint64_t large_id = (std::uint64_t(1) << 40) + 1;
                auto                    large    = Completion::make().unwrap();
                auto                    low      = Completion::make().unwrap();
                require(pending.add(large_id, rstd::move(large.get<1>())), "large id rejected");
                require(pending.add(1, rstd::move(low.get<1>())), "large id truncated");
                require(pending.complete(large_id, "large"), "large response was lost");
                require(pending.complete(1, "low"), "low response was lost");
                auto large_response = co_await rstd::move(large.get<0>());
                auto low_response   = co_await rstd::move(low.get<0>());
                require(large_response.is_ok() && large_response.unwrap() == "large",
                        "large id matched the wrong response");
                require(low_response.is_ok() && low_response.unwrap() == "low",
                        "low id matched the wrong response");

                auto rejected_send = Completion::make().unwrap();
                require(pending.add(9, rstd::move(rejected_send.get<1>())), "send id rejected");
                canceler.fail(9, QStringLiteral("send rejected"));
                auto sent = co_await rstd::move(rejected_send.get<0>());
                require(sent.is_err() && sent.unwrap_err().is_failed(),
                        "send rejection did not finish the request");

                auto detached = Completion::make().unwrap();
                auto retained = [&] {
                    waywallen::BackendPending<QByteArray, QString> owner;
                    require(owner.add(10, rstd::move(detached.get<1>())), "detached id rejected");
                    return owner.clone();
                }();
                retained.cancel(10);
                auto released = co_await rstd::move(detached.get<0>());
                require(released.is_err() && released.unwrap_err().is_canceled(),
                        "cancellation required the original owner");
            } catch (...) {
                failure = std::current_exception();
            }
        };
        runtime.block_on(run());
        if (failure) std::rethrow_exception(failure);
        std::puts("pending cancellation, late responses and one-shot completion passed");
    } catch (const std::exception& error) {
        std::fprintf(stderr, "backend pending: %s\n", error.what());
        return 1;
    }
}
