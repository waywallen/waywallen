module;

export module waywallen:model.share_store;
export import qextra;
import rstd.cppstd;

export namespace waywallen
{

template<typename T>
class ShareStore : public kstore::ShareStore<T, cppstd::pmr::polymorphic_allocator<T>> {
public:
    using base_type = kstore::ShareStore<T, cppstd::pmr::polymorphic_allocator<T>>;
    ShareStore(): base_type() {}
};

} // namespace waywallen
