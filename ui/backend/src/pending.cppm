module;
#include <cstdint>

export module waywallen.backend.connection:pending;
import rstd;

export namespace waywallen
{
template<class Message, class Error>
class BackendPending {
public:
    using Handle = rstd::async::CompletionHandle<Message, Error>;

    auto clone() const -> BackendPending { return BackendPending(state_.clone()); }

    BackendPending(): state_(rstd::sync::Arc<State>::make()) {}

    auto contains(std::uint64_t id) const -> bool {
        return state_->responses.lock().unwrap()->contains_key(rstd::u64(id));
    }

    auto add(std::uint64_t id, Handle response) -> bool {
        auto responses = state_->responses.lock().unwrap();
        if (responses->contains_key(rstd::u64(id))) return false;
        responses->insert(rstd::u64(id), rstd::move(response));
        return true;
    }

    auto complete(std::uint64_t id, Message message) -> bool {
        auto response = take(id);
        if (response.is_none()) return false;
        (void)response->complete(rstd::move(message));
        return true;
    }

    void cancel(std::uint64_t id) { take(id); }

    void fail(std::uint64_t id, const Error& error) {
        if (auto response = take(id)) (void)response->fail(error);
    }

    void fail_all(const Error& error) {
        auto responses = [&] {
            auto locked  = state_->responses.lock().unwrap();
            auto entries = rstd::move(*locked).into_iter();
            *locked      = Map::make();
            return entries;
        }();
        for (auto entry = responses.next(); entry.is_some(); entry = responses.next()) {
            (void)entry->template get<1>().fail(error);
        }
    }

private:
    using Map = rstd::collections::HashMap<rstd::u64, Handle>;
    struct State {
        rstd::sync::Mutex<Map> responses { Map::make() };
    };
    explicit BackendPending(rstd::sync::Arc<State> state): state_(rstd::move(state)) {}

    auto take(std::uint64_t id) -> rstd::Option<Handle> {
        return state_->responses.lock().unwrap()->remove(rstd::u64(id));
    }

    rstd::sync::Arc<State> state_;
};
} // namespace waywallen
