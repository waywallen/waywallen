#include <rstd/test/gtest.hpp>
import rstd;
import rstd.test;
import waywallen.image.content;

using namespace rstd::prelude;
using namespace rstd::literals;
using rstd::path::Path;
using rstd::path::PathBuf;
using rstd::time::Duration;
using rstd::time::SystemTime;

namespace
{
struct Package {
    rstd::fs::TempDir storage;
    PathBuf           root;
    Package()
        : storage(rstd::fs::TempDir::make("waywallen-image"_str).unwrap()),
          root(PathBuf::from(storage.path())) {}
    auto path(ref<str> relative) const -> PathBuf { return root.join(ref<Path>(relative)); }
    void write(ref<str> relative, ref<str> value) {
        rstd::fs::write(path(relative), value.as_bytes()).unwrap();
    }
    void image(const char* variant, unsigned char r, unsigned char g, unsigned char b,
               int width = 2, int height = 2, const char* name = "2x2.png") {
        auto directory = path(rstd::format("contents/{}", variant));
        rstd::fs::create_dir_all(directory).unwrap();
        auto header = rstd::format("P6\n{} {}\n255\n", width, height);
        auto bytes  = Vec<u8>::from(header.as_str().as_bytes());
        for (int i = 0; i < width * height; ++i) {
            bytes.push(u8(r));
            bytes.push(u8(g));
            bytes.push(u8(b));
        }
        auto filename = rstd::ffi::CStr::from_ptr(name).to_str().unwrap();
        rstd::fs::write(directory.join(ref<Path>(filename)), bytes.as_slice()).unwrap();
    }
};

TEST(ImageContent, SimulatedSchedule) {
    using ww_image::ImageContent;
    ASSERT_EQ(ImageContent::simulated_darkness(0, true), 1.0);
    ASSERT_EQ(ImageContent::simulated_darkness(6 * 3600, true), 1.0);
    ASSERT_EQ(ImageContent::simulated_darkness(6 * 3600 + 900, true), 0.5);
    ASSERT_EQ(ImageContent::simulated_darkness(12 * 3600, true), 0.0);
    ASSERT_EQ(ImageContent::simulated_darkness(18 * 3600 + 900, true), 0.5);
    ASSERT_EQ(ImageContent::simulated_darkness(23 * 3600, true), 1.0);
    ASSERT_EQ(ImageContent::simulated_darkness(6 * 3600, false), 0.0);
    ASSERT_EQ(ImageContent::simulated_darkness(18 * 3600, false), 1.0);
}

TEST(ImageContent, PackageModesAndMissingSide) {
    Package package;
    package.image("images", 255, 0, 0);
    package.image("images_dark", 0, 0, 255);
    ww_image::DecodeError error;
    auto content = ww_image::ImageContent::open(package.root.as_path().to_str().unwrap(), &error);
    ASSERT_TRUE(bool(content));
    ASSERT_TRUE(content->dynamic());
    content->set_mode("light"_str);
    auto light = content->decode(2, 2, &error);
    ASSERT_EQ(light.data.len().to_primitive(), rstd::size_t(16));
    ASSERT_EQ(light.data[usize(0)], u8(255));
    ASSERT_EQ(light.data[usize(2)], u8(0));
    content->set_mode("dark"_str);
    auto dark = content->decode(2, 2, &error);
    ASSERT_EQ(dark.data[usize(0)], u8(0));
    ASSERT_EQ(dark.data[usize(2)], u8(255));
    ASSERT_TRUE(! content->dynamic());
    rstd::fs::remove_dir_all(package.path("contents/images_dark"_str)).unwrap();
    auto fallback = content->decode(2, 2, &error);
    ASSERT_EQ(fallback.data[usize(0)], u8(255));
}

TEST(ImageContent, PlainImageAndEmptyPackage) {
    Package               package;
    ww_image::DecodeError error;
    ASSERT_TRUE(! ww_image::ImageContent::open(package.root.as_path().to_str().unwrap(), &error));
    package.image("images_dark", 0, 0, 255);
    ASSERT_TRUE(! ww_image::ImageContent::open(package.root.as_path().to_str().unwrap(), &error));
    package.image("images", 0, 0, 255);
    rstd::fs::remove_dir_all(package.path("contents/images_dark"_str)).unwrap();
    auto content = ww_image::ImageContent::open(package.root.as_path().to_str().unwrap(), &error);
    ASSERT_TRUE(bool(content));
    ASSERT_TRUE(! content->dynamic());
    auto image = content->decode(4, 4, &error);
    ASSERT_EQ(image.data.len().to_primitive(), rstd::size_t(64));
    ASSERT_EQ(image.data[usize(2)], u8(255));
    auto plain = ww_image::ImageContent::open(
        package.path("contents/images/2x2.png"_str).as_path().to_str().unwrap(), &error);
    ASSERT_TRUE(bool(plain));
    ASSERT_TRUE(! plain->dynamic());
    ASSERT_EQ(plain->width(), 2u);
}

TEST(ImageContent, CrossfadeMetadataAndClockChanges) {
    Package package;
    package.image("images", 255, 0, 0);
    package.image("images_dark", 0, 0, 255);
    ww_image::DecodeError error;
    using namespace rstd::time;
    auto local =
        PlainDateTime::make(Date::from_calendar_date(i32(2026), Month::June, u8(15)).unwrap(),
                            Time::from_hms_nano(u8(6), u8(15), u8(), u32()).unwrap());
    auto       offset    = UtcOffset::local_offset_at(local.assume_utc()).unwrap();
    auto       timestamp = local.assume_offset(offset).unix_time();
    const auto morning =
        SystemTime::from_unix_time(timestamp.seconds, timestamp.nanoseconds).unwrap();
    auto content = ww_image::ImageContent::open(package.root.as_path().to_str().unwrap(), &error);
    content->update(morning);
    auto mixed = content->decode(2, 2, &error);
    ASSERT_EQ(mixed.data[usize(0)], u8(128));
    ASSERT_EQ(mixed.data[usize(2)], u8(128));
    content->update(morning + Duration::from_secs(u64(12 * 3600)));
    ASSERT_EQ(content->decode(2, 2, &error).data[usize(0)], u8(128));
    content->update(morning + Duration::from_secs(u64(6 * 3600)));
    ASSERT_EQ(content->decode(2, 2, &error).data[usize(0)], u8(255));
    content->update(morning);
    ASSERT_EQ(content->decode(2, 2, &error).data[usize(0)], u8(128));
    package.write("metadata.json"_str, R"({"X-KDE-CrossFade":false})"_str);
    auto discrete = ww_image::ImageContent::open(package.root.as_path().to_str().unwrap(), &error);
    discrete->update(morning);
    ASSERT_EQ(discrete->decode(2, 2, &error).data[usize(0)], u8(255));
    discrete->update(morning + Duration::from_secs(u64(12 * 3600)));
    ASSERT_EQ(discrete->decode(2, 2, &error).data[usize(2)], u8(255));
}

TEST(ImageContent, SelectsResolutionAndFallsBackAfterDecodeFailure) {
    Package package;
    package.image("images", 255, 0, 0, 2, 2, "2x2.png");
    package.image("images", 0, 0, 255, 8, 8, "8x8.png");
    ww_image::DecodeError error;
    auto content = ww_image::ImageContent::open(package.root.as_path().to_str().unwrap(), &error);
    ASSERT_TRUE(bool(content));
    ASSERT_EQ(content->width(), 8u);
    ASSERT_EQ(content->decode(2, 2, &error).data[usize(0)], u8(255));
    ASSERT_EQ(content->decode(5, 5, &error).data[usize(2)], u8(255));
    package.write("contents/images/8x8.png"_str, "broken"_str);
    auto fallback = content->decode(5, 5, &error);
    ASSERT_EQ(fallback.data.len().to_primitive(), rstd::size_t(100));
    ASSERT_EQ(fallback.data[usize(0)], u8(255));
}

TEST(ImageContent, KdeFilenameAndPreferredSizeRules) {
    Package package;
    package.image("images", 255, 0, 0, 2, 2, "light.png");
    package.image("images", 255, 0, 0, 2, 2, "0x100.png");
    package.image("images", 255, 0, 0, 2, 2, "2147483648x1.png");
    ww_image::DecodeError error;
    ASSERT_TRUE(! ww_image::ImageContent::open(package.root.as_path().to_str().unwrap(), &error));
    package.image("images", 255, 0, 0, 2, 2, "1920x1080.png");
    package.image("images", 0, 0, 255, 2, 2, "1280x1024.png");
    package.image("images", 0, 255, 0, 2, 2, "1600x1200.png");
    auto content = ww_image::ImageContent::open(package.root.as_path().to_str().unwrap(), &error);
    ASSERT_TRUE(bool(content));
    ASSERT_EQ(content->decode(1350, 1080, &error).data[usize(2)], u8(255));
    ASSERT_EQ(content->decode(1440, 1080, &error).data[usize(1)], u8(255));
    ASSERT_EQ(content->decode(2048, 1080, &error).data[usize(0)], u8(255));
}

TEST(ImageContent, VariantSizesAndNamesNeedNotMatch) {
    Package package;
    package.image("images", 255, 0, 0);
    package.image("images_dark", 0, 0, 255, 4, 2, "4x2.png");
    ww_image::DecodeError error;
    auto content = ww_image::ImageContent::open(package.root.as_path().to_str().unwrap(), &error);
    ASSERT_TRUE(bool(content));
    ASSERT_TRUE(content->dynamic());
    content->set_mode("dark"_str);
    ASSERT_EQ(content->decode(2, 2, &error).data[usize(2)], u8(255));
}

TEST(ImageContent, PackageFollowsSymlinkedImages) {
    Package package;
    package.image("source", 255, 0, 0);
    rstd::fs::create_dir_all(package.path("contents/images"_str)).unwrap();
    rstd::fs::soft_link(package.path("contents/source/2x2.png"_str),
                        package.path("contents/images/1920x1080.png"_str))
        .unwrap();
    ww_image::DecodeError error;
    auto content = ww_image::ImageContent::open(package.root.as_path().to_str().unwrap(), &error);
    ASSERT_TRUE(bool(content));
    ASSERT_EQ(content->decode(2, 2, &error).data[usize()], u8(255));
}
} // namespace
