#include "waywallen/model/grid_model.hpp"
#include "qml_material/model/item_model_source.hpp"
#include <QCoreApplication>
#include <array>

import qextra;

struct GridRow {
    QString key;
    int     acquisitionState;
    bool    operator==(const GridRow&) const = default;
};
Q_DECLARE_METATYPE(GridRow)

template<>
struct kstore::ItemTrait<GridRow> {
    using key_type   = QString;
    using store_type = kstore::ShareStore<GridRow>;
    static auto key(const GridRow& row) -> QString { return row.key; }
};

class GridModel final
    : public waywallen::model::GridListModel,
      public kstore::QMetaListModelCRTP<GridRow, GridModel, kstore::ListStoreType::Share> {
public:
    GridModel(): GridListModel(this) { setSelectionEnabled(true); }
};

static void check(bool condition, const char* message) {
    if (! condition) qFatal("%s", message);
}

int main(int argc, char** argv) {
    QCoreApplication            app(argc, argv);
    kstore::ShareStore<GridRow> store;
    GridModel                   model;
    model.set_store(&model, store);
    model.insert(0,
                 std::array { GridRow { QString::fromUtf8("wallhaven\0portrait", 18), 0 },
                              GridRow { QStringLiteral("other"), 0 } });

    qml_material::ItemModelSource source;
    source.setKeyRole(QStringLiteral("itemKey"));
    source.setRoles({ QStringLiteral("item"), QStringLiteral("selected") });
    source.setModel(&model);
    check(source.errorString().isEmpty(), "Typed model projection failed");
    const auto first = source.snapshot();
    check(first->count() == 2, "Typed model row count differs");
    const auto key = model.rawKeyAt(0).toString();
    check(first->descriptorAt(0).key == key, "Key role did not preserve the trait key");
    check(first->valueAt(0).toMap().value("item").value<GridRow>().acquisitionState == 0,
          "Typed item was not projected");

    model.setSelected(0, true);
    const auto selected = source.snapshot();
    check(selected->valueAt(0).toMap().value("selected").toBool(),
          "Selection change did not reach the source");
    check(selected->descriptorAt(0).key == key, "Selection changed item identity");
    check(! first->valueAt(0).toMap().value("selected").toBool(),
          "Published snapshot changed after selection");

    const auto [handle, changed] = store.store_insert(GridRow { key, 3 });
    check(changed, "Store did not accept the state change");
    store.store_changed_callback(std::array { key });
    const auto acquired = source.snapshot();
    check(acquired->valueAt(0).toMap().value("item").value<GridRow>().acquisitionState == 3,
          "Shared acquisition state did not reach the source");
    check(first->valueAt(0).toMap().value("item").value<GridRow>().acquisitionState == 0,
          "Published typed value changed in place");
    check(acquired->descriptorAt(0).contentRevision > selected->descriptorAt(0).contentRevision,
          "Content revision was not advanced");
    model.remove(0);
    check(source.count() == 1 && source.snapshot()->descriptorAt(0).key == "other",
          "Removal did not retain the surviving identity");
}
