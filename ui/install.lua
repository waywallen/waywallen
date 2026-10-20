local releases = lito.read_file("assets/waywallen-ui.releases.xml")

lito.install({
  artifacts = {
    {
      target = { kind = "bin", name = "waywallen-ui" },
      destination = "bin/waywallen-ui",
    },
  },
  external_assets = {
    {
      dependency = "waywallen-daemon",
      set = "waywallen",
      destination = "bin",
    },
  },
  generated_files = {
    {
      target = { kind = "bin", name = "waywallen-ui" },
      source = "lito-qml/waywallen/ui",
      destination = "share/qml/waywallen/ui",
    },
  },
  files = {
    {
      source = "assets/waywallen-ui.svg",
      destination = "share/icons/hicolor/scalable/apps/org.waywallen.waywallen.svg",
    },
  },
  templates = {
    {
      input = "assets/waywallen-ui.desktop.in",
      destination = "share/applications/org.waywallen.waywallen.desktop",
      values = {
        APP_ID = "org.waywallen.waywallen",
        APP_SUMMARY = "Wallpaper Manager for Linux",
      },
    },
    {
      input = "assets/waywallen-ui.metainfo.xml.in",
      destination = "share/metainfo/org.waywallen.waywallen.metainfo.xml",
      values = {
        APP_ID = "org.waywallen.waywallen",
        APP_NAME = "waywallen",
        APP_SUMMARY = "Wallpaper Manager for Linux",
        APP_AUTHOR = "hypengw",
        APP_RELEASES = releases,
      },
    },
  },
})
