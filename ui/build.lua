local lito = require("@lito")
local qt = require("@lito.qt")

local ui = lito.target({ kind = "bin", name = "waywallen-ui" })
local qt6 = lito.external_dependency(ui, "qt6")
local control_source = lito.external_source(ui, "waywallen-control")

qt.protobuf({
  target = ui,
  qt = qt6,
  source = control_source,
  proto_files = {
    "control.proto",
    "filter.proto",
  },
  proto_includes = { "." },
  output = "lito-protobuf/waywallen_control",
  qml_uri = "waywallen.control",
  qml_version = "1.0",
})

qt.translations({
  target = ui,
  qt = qt6,
  name = "waywallen_ui",
  ts_files = {
    "i18n/waywallen_zh_CN.ts",
    "i18n/waywallen_ru.ts",
  },
  resource_prefix = "/i18n",
})

local qml_module = {
  target = ui,
  qt = qt6,
  uri = "waywallen.ui",
  version = "0.3",
  output = "lito-qml/waywallen/ui",
  resource_prefix = "/",
  qml_files = {
    "qml/Window.qml",
    "qml/Global.qml",
    "qml/I18n.qml",
    "qml/component/StatusDot.qml",
    "qml/component/PagePopup.qml",
    "qml/component/Tag.qml",
    "qml/component/GpuDetails.qml",
    "qml/component/GpuTag.qml",
    "qml/component/RendererRuntimeTag.qml",
    "qml/component/RuntimeConditionTag.qml",
    "qml/component/KdeDisplaysHelp.qml",
    "qml/component/GnomeDisplaysHelp.qml",
    "qml/component/LayerShellDisplaysHelp.qml",
    "qml/component/ThumbnailImage.qml",
    "qml/component/SearchChip.qml",
    "qml/component/ValueSlider.qml",
    "qml/component/DetailActionBar.qml",
    "qml/component/PresentationTargetState.qml",
    "qml/component/PresentationTargetFlow.qml",
    "qml/component/filter/StringFilter.qml",
    "qml/component/filter/IntFilter.qml",
    "qml/component/filter/WpTypeFilter.qml",
    "qml/component/filter/TagFilter.qml",
    "qml/component/filter/TagPickerDialog.qml",
    "qml/component/filter/ContentRatingFilter.qml",
    "qml/component/filter/EmptyFilter.qml",
    "qml/component/filter/WallpaperFilter.qml",
    "qml/component/settings/SettingField.qml",
    "qml/dialog/DaemonNotRunDialog.qml",
    "qml/dialog/WallpaperFilterDialog.qml",
    "qml/dialog/RemoteFilterDialog.qml",
    "qml/dialog/QrLoginDialog.qml",
    "qml/dialog/PluginActionFormDialog.qml",
    "qml/dialog/DisplayEditDialog.qml",
    "qml/dialog/CanvasDialog.qml",
    "qml/dialog/DaemonLogDialog.qml",
    "qml/page/WallpaperPage.qml",
    "qml/page/PlaylistEditPage.qml",
    "qml/page/WallpaperDetailPanel.qml",
    "qml/page/DiscoverState.qml",
    "qml/page/RemoteDetailPanel.qml",
    "qml/page/WallpaperInfoPage.qml",
    "qml/page/WallpaperCard.qml",
    "qml/page/wallpaper/SelectSheet.qml",
    "qml/page/wallpaper/TweakSheet.qml",
    "qml/page/wallpaper/TweakState.qml",
    "qml/page/wallpaper/NewPlaylistSheetContent.qml",
    "qml/page/wallpaper/AddToPlaylistSheetContent.qml",
    "qml/page/wallpaper/SelectSheetContentState.qml",
    "qml/page/wallpaper/PlaylistListSheet.qml",
    "qml/page/wallpaper/PlaylistListSheetState.qml",
    "qml/page/DiscoverPage.qml",
    "qml/page/RemoteInfoPage.qml",
    "qml/page/RemoteManagePage.qml",
    "qml/page/RemoteCard.qml",
    "qml/page/StatusPage.qml",
    "qml/page/PluginManagePage.qml",
    "qml/page/SettingsPage.qml",
    "qml/page/DisplaysPage.qml",
    "qml/page/display/CanvasEditorState.qml",
    "qml/page/display/CanvasDisplayList.qml",
    "qml/page/display/DisplayLayoutControls.qml",
    "qml/page/display/RendererConnectionPanel.qml",
    "qml/page/SourceManagePage.qml",
    "qml/page/AddLibraryPage.qml",
    "qml/page/AboutPage.qml",
    "qml/page/PluginSettingsPage.qml",
  },
  singletons = {
    "qml/Global.qml",
    "qml/I18n.qml",
    "qml/component/GpuDetails.qml",
  },
  resources = {
    "assets/waywallen-ui.svg",
    "assets/waywallen-ui.releases.xml",
  },
  imports = {
    "QtCore",
    "QtQuick",
    "QtQuick.Controls",
    "QtQuick.Shapes",
    "QtQml.Models",
    "Qcm.Material",
    "waywallen.control",
  },
  moc_files = {
    { source = "src/action.cppm", mode = "module-split", output = "waywallen/action" },
    { source = "src/util.cppm", mode = "module-split", output = "waywallen/util" },
    { source = "src/app.cppm", mode = "module-split", output = "waywallen/app" },
    { source = "src/backend.cppm", mode = "module-split", output = "waywallen/backend" },
    {
      source = "src/daemon_dbus.cppm",
      mode = "module-split",
      output = "waywallen/daemon_dbus",
    },
    {
      source = "src/objmodel/display.cppm",
      mode = "module-split",
      output = "waywallen/objmodel/display",
    },
    {
      source = "src/objmodel/gpu.cppm",
      mode = "module-split",
      output = "waywallen/objmodel/gpu",
    },
    {
      source = "src/objmodel/library.cppm",
      mode = "module-split",
      output = "waywallen/objmodel/library",
    },
    { source = "src/notify.cppm", mode = "module-split", output = "waywallen/notify" },
    {
      source = "src/plugin_translation.cppm",
      mode = "module-split",
      output = "waywallen/plugin_translation",
    },
    {
      source = "src/objmodel/renderer.cppm",
      mode = "module-split",
      output = "waywallen/objmodel/renderer",
    },
    { source = "src/msg/store.cppm", mode = "module-split", output = "waywallen/msg/store" },
    {
      source = "src/model/store_item.cppm",
      mode = "module-split",
      output = "waywallen/model/store_item",
    },
    {
      source = "src/model/list_models.cppm",
      mode = "module-split",
      output = "waywallen/model/list_models",
    },
    {
      source = "src/model/wallpaper_select_storage.cppm",
      mode = "module-split",
      output = "waywallen/model/wallpaper_select_storage",
    },
    {
      source = "src/model/remote_model.cppm",
      mode = "module-split",
      output = "waywallen/model/remote_model",
    },
    {
      source = "src/model/filter_rule_model.cppm",
      mode = "module-split",
      output = "waywallen/model/filter_rule_model",
    },
    {
      source = "src/model/user_property_model.cppm",
      mode = "module-split",
      output = "waywallen/model/user_property_model",
    },
    {
      source = "src/query/wallpaper_query.cppm",
      mode = "module-split",
      output = "waywallen/query/wallpaper_query",
    },
    {
      source = "src/query/progress_query.cppm",
      mode = "module-split",
      output = "waywallen/query/progress_query",
    },
    {
      source = "src/query/renderer_query.cppm",
      mode = "module-split",
      output = "waywallen/query/renderer_query",
    },
    {
      source = "src/query/source_query.cppm",
      mode = "module-split",
      output = "waywallen/query/source_query",
    },
    {
      source = "src/query/plugin_query.cppm",
      mode = "module-split",
      output = "waywallen/query/plugin_query",
    },
    {
      source = "src/query/tag_query.cppm",
      mode = "module-split",
      output = "waywallen/query/tag_query",
    },
    {
      source = "src/query/health_query.cppm",
      mode = "module-split",
      output = "waywallen/query/health_query",
    },
    {
      source = "src/query/log_query.cppm",
      mode = "module-split",
      output = "waywallen/query/log_query",
    },
    {
      source = "src/query/display_query.cppm",
      mode = "module-split",
      output = "waywallen/query/display_query",
    },
    {
      source = "src/query/gpu_query.cppm",
      mode = "module-split",
      output = "waywallen/query/gpu_query",
    },
    {
      source = "src/query/library_query.cppm",
      mode = "module-split",
      output = "waywallen/query/library_query",
    },
    {
      source = "src/query/settings_query.cppm",
      mode = "module-split",
      output = "waywallen/query/settings_query",
    },
    {
      source = "src/query/autostart_query.cppm",
      mode = "module-split",
      output = "waywallen/query/autostart_query",
    },
    {
      source = "src/query/remote_query.cppm",
      mode = "module-split",
      output = "waywallen/query/remote_query",
    },
    {
      source = "src/query/playlist_query.cppm",
      mode = "module-split",
      output = "waywallen/query/playlist_query",
    },
    {
      source = "src/query/qr_login_query.cppm",
      mode = "module-split",
      output = "waywallen/query/qr_login_query",
    },
    {
      source = "src/query/plugin_action_query.cppm",
      mode = "module-split",
      output = "waywallen/query/plugin_action_query",
    },
    {
      source = "src/query/global_pause_query.cppm",
      mode = "module-split",
      output = "waywallen/query/global_pause_query",
    },
    {
      source = "src/thumb/service.cppm",
      mode = "module-split",
      output = "waywallen/thumb/service",
    },
    {
      source = "include/waywallen/register/qml_register.hpp",
      mode = "separate",
      output = "waywallen/register/moc_qml_register.cpp",
      compile = false,
    },
  },
}

qt.qml_module(qml_module)

-- Copy the QML sources next to the generated qmldir so the module directory is
-- a complete QML module on disk. This makes
-- build/<profile>/generated/waywallen-ui/lito-qml a usable QML import path, so
-- qmllint and qmlls can resolve waywallen.ui when checking ui/qml.
for _, path in ipairs(qml_module.qml_files) do
  lito.target_add_metadata(ui, lito.copy({
    input = path,
    output = qml_module.output .. "/" .. path,
  }).output)
end
