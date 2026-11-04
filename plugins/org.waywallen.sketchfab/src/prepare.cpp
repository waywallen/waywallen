module;
#include <rstd/macro.hpp>

module waywallen.sketchfab.prepare;
import rstd;
import rstd.json;
import licompto;

using namespace rstd::prelude;
using namespace rstd::literals;
using rstd::path::Path;
using rstd::path::PathBuf;
using Json      = rstd::json::Value;
using JsonMap   = rstd::json::Map;
using JsonArray = rstd::json::Array;
template<class T>
using Prepared = Result<T, String>;

namespace waywallen::sketchfab
{
namespace
{
constexpr unsigned max_face   = 1024;
constexpr unsigned max_brdf   = 1024;
constexpr unsigned max_input  = 64 * 1024 * 1024;
constexpr unsigned max_output = 128 * 1024 * 1024;
constexpr double   pi         = 3.14159265358979323846;

struct Cube {
    String   resource;
    unsigned face {};
    unsigned limit {};
};
struct Brdf {
    String   resource;
    unsigned width {}, height {};
};
struct Request {
    Option<Cube>           background, specular;
    Option<Brdf>           brdf;
    Option<array<f64, 27>> sh;
    JsonMap                resources;
    String                 prefix;
};
struct Rgb {
    double r {}, g {}, b {};
};
struct Direction {
    double x {}, y {}, z {};
};
struct FaceUv {
    unsigned face {};
    double   u {}, v {};
};

auto fail(ref<str> text) -> String { return String::make(text); }
auto jstring(ref<str> text) -> Json { return Json::String(String::make(text)); }
auto jnumber(unsigned value) -> Json {
    return Json::Number(rstd::json::Number::from_u64(u64(value)));
}
auto jfloat(double value) -> Json {
    return Json::Number(rstd::json::Number::from_f64(f64(value)).unwrap());
}
void put(JsonMap& map, ref<str> key, Json value) {
    map.insert(String::make(key), rstd::move(value));
}
auto integer(const Json& object, ref<str> key, unsigned minimum, unsigned maximum)
    -> Prepared<unsigned> {
    auto field = object.get(key);
    auto value = field ? (*field)->as_u64() : None<u64>();
    if (! value || *value < u64(minimum) || *value > u64(maximum))
        return Err(rstd::format("{} must be an integer in {}..{}", key, minimum, maximum));
    return Ok(unsigned(value->to_primitive()));
}
auto string(const Json& object, ref<str> key) -> Prepared<String> {
    auto field = object.get(key);
    auto value = field ? (*field)->as_str() : None<ref<str>>();
    if (! value || value->is_empty()) return Err(rstd::format("{} must be a nonempty string", key));
    return Ok(String::make(*value));
}
auto power_two(unsigned value) -> bool { return value && ! (value & (value - 1)); }
auto safe_prefix(ref<str> value) -> bool {
    if (value.is_empty() || value.len() > usize(64)) return false;
    for (auto byte : value.as_bytes()) {
        auto c = byte.to_primitive();
        if (! ((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') ||
               c == '_' || c == '-'))
            return false;
    }
    return true;
}
auto normal_relative(ref<str> text) -> bool {
    if (text.is_empty() || text.len() > usize(4096)) return false;
    usize segment {};
    bool  dots = true;
    for (auto byte : text.as_bytes()) {
        if (byte == u8(0) || byte == u8('\\') || byte == u8(':')) return false;
        if (byte == u8('/')) {
            if (segment == usize(0) || (dots && segment <= usize(2))) return false;
            segment = usize(0);
            dots    = true;
        } else {
            ++segment;
            dots = dots && byte == u8('.');
        }
    }
    if (segment == usize(0) || (dots && segment <= usize(2))) return false;
    auto path = PathBuf::from(text);
    if (! path.as_path().is_safe_relative()) return false;
    for (auto part : path.as_path().components())
        if (! part.is_normal()) return false;
    return true;
}
auto parse_cube(const Json& value, bool specular) -> Prepared<Cube> {
    auto resource = string(value, "resource"_str);
    if (resource.is_err()) return Err(resource.unwrap_err());
    auto size = integer(value, "face_size"_str, 1, max_face);
    if (size.is_err()) return Err(size.unwrap_err());
    auto face = size.unwrap();
    if (! power_two(face)) return Err(fail("face_size must be a power of two"_str));
    unsigned limit = 1;
    if (specular) {
        auto parsed = integer(value, "limit_size"_str, 1, face);
        if (parsed.is_err()) return Err(parsed.unwrap_err());
        limit = parsed.unwrap();
        if (! power_two(limit)) return Err(fail("limit_size must be a power of two"_str));
    }
    return Ok(Cube { resource.unwrap(), face, limit });
}
auto parse_request(Json value) -> Prepared<Request> {
    auto version = integer(value, "version"_str, 1, 1);
    if (version.is_err()) return Err(version.unwrap_err());
    auto parameters = value.get("parameters"_str);
    auto resources  = value.get("resources"_str);
    if (! parameters || ! (*parameters)->is_object() || ! resources || ! (*resources)->is_object())
        return Err(fail("parameters and resources must be objects"_str));
    auto pversion = integer(**parameters, "version"_str, 1, 1);
    if (pversion.is_err()) return Err(pversion.unwrap_err());
    auto prefix = string(**parameters, "output_prefix"_str);
    if (prefix.is_err()) return Err(prefix.unwrap_err());
    auto prefix_name = prefix.unwrap();
    if (! safe_prefix(prefix_name.as_str()))
        return Err(fail("output_prefix must be a single ASCII name"_str));
    Request request;
    request.prefix        = rstd::move(prefix_name);
    auto source_resources = (*resources)->as_object();
    if ((**source_resources).len() > usize(16)) return Err(fail("too many resources"_str));
    for (auto [id, path] : (**source_resources).iter()) {
        auto name = path->as_str();
        if (id->is_empty() || ! name || ! normal_relative(*name))
            return Err(fail("resource names must be normal relative paths"_str));
        put(request.resources, id->as_str(), jstring(*name));
    }
    if (auto field = (*parameters)->get("background"_str)) {
        auto parsed = parse_cube(**field, false);
        if (parsed.is_err()) return Err(parsed.unwrap_err());
        request.background = Some(parsed.unwrap());
    }
    if (auto field = (*parameters)->get("specular"_str)) {
        auto parsed = parse_cube(**field, true);
        if (parsed.is_err()) return Err(parsed.unwrap_err());
        request.specular = Some(parsed.unwrap());
    }
    if (auto field = (*parameters)->get("brdf"_str)) {
        auto resource = string(**field, "resource"_str);
        if (resource.is_err()) return Err(resource.unwrap_err());
        auto width = integer(**field, "width"_str, 1, max_brdf);
        if (width.is_err()) return Err(width.unwrap_err());
        auto height = integer(**field, "height"_str, 1, max_brdf);
        if (height.is_err()) return Err(height.unwrap_err());
        request.brdf = Some(Brdf { resource.unwrap(), width.unwrap(), height.unwrap() });
    }
    if (auto field = (*parameters)->get("diffuse_sh"_str)) {
        auto values = (*field)->as_array();
        if (! values || (**values).len() < usize(27) || (**values).len() > usize(256))
            return Err(fail("diffuse_sh must contain 27..256 finite coefficients"_str));
        array<f64, 27> sh;
        for (usize i {}; i < (**values).len(); ++i) {
            auto number = (**values)[i].as_f64();
            if (! number || ! number->is_finite() || number->abs() > f64(1e20))
                return Err(fail("diffuse_sh coefficient is invalid"_str));
            if (i < usize(27)) sh[i] = *number;
        }
        request.sh = Some(rstd::move(sh));
    }
    return Ok(rstd::move(request));
}

auto checked_input(const PathBuf& root, ref<str> relative) -> Prepared<PathBuf> {
    if (! normal_relative(relative)) return Err(fail("invalid input resource path"_str));
    auto path  = root.clone();
    auto child = PathBuf::from(relative);
    for (auto component : child.as_path().components()) {
        path          = path.join(PathBuf::from(component.as_os_str().to_string_lossy()).as_path());
        auto metadata = rstd::fs::symlink_metadata(path.as_path());
        if (metadata.is_err())
            return Err(rstd::format("input metadata: {}", metadata.unwrap_err()));
        if (metadata.unwrap().is_symlink()) return Err(fail("input symlinks are forbidden"_str));
    }
    auto metadata = rstd::fs::metadata(path.as_path());
    if (metadata.is_err()) return Err(rstd::format("input metadata: {}", metadata.unwrap_err()));
    if (! metadata.unwrap().is_file()) return Err(fail("input must be a regular file"_str));
    return Ok(rstd::move(path));
}
auto read_file(const PathBuf& path, unsigned limit) -> Prepared<Vec<u8>> {
    auto opened = rstd::fs::File::open(path.as_path());
    if (opened.is_err()) return Err(rstd::format("input open: {}", opened.unwrap_err()));
    auto file     = opened.unwrap();
    auto metadata = file.metadata();
    if (metadata.is_err()) return Err(rstd::format("input metadata: {}", metadata.unwrap_err()));
    auto size = metadata.unwrap().len();
    if (size > u64(limit)) return Err(fail("input exceeds byte limit"_str));
    Vec<u8> bytes;
    bytes.resize(usize(size.to_primitive()), u8(0));
    auto read = file.read_exact_at(bytes.as_mut_slice().as_mut_ref(), u64(0));
    if (read.is_err()) return Err(rstd::format("input read: {}", read.unwrap_err()));
    return Ok(rstd::move(bytes));
}
auto decode_bytes(Vec<u8> bytes, unsigned expected) -> Prepared<Vec<u8>> {
    if (bytes.len() >= usize(2) && bytes[usize(0)] == u8(0x1f) && bytes[usize(1)] == u8(0x8b)) {
        auto decoded = licompto::decompress<licompto::GzipDecoder>(
            bytes.as_slice(), { { u64(expected), u64(65536), u64(1) } });
        if (decoded.is_err()) return Err(rstd::format("gzip: {}", decoded.unwrap_err()));
        bytes = decoded.unwrap();
    }
    if (bytes.len() != usize(expected)) return Err(fail("resource layout length mismatch"_str));
    return Ok(rstd::move(bytes));
}
auto load_resource(const Request& request, const PathBuf& root, ref<str> id, unsigned expected)
    -> Prepared<Vec<u8>> {
    auto field = request.resources.get(id);
    if (! field) return Err(rstd::format("missing resource {}", id));
    auto relative = (*field)->as_str();
    auto path     = checked_input(root, *relative);
    if (path.is_err()) return Err(path.unwrap_err());
    auto bytes = read_file(path.unwrap(), max_input);
    if (bytes.is_err()) return Err(bytes.unwrap_err());
    return decode_bytes(bytes.unwrap(), expected);
}

auto direction(unsigned x, unsigned y, unsigned width, unsigned height) -> Direction {
    double theta     = (double(y) + 0.5) / height * pi;
    double phi       = ((double(x) + 0.5) / width - 0.5) * (2 * pi);
    auto   sin_theta = f64(theta).sin().to_primitive();
    return { sin_theta * f64(phi).cos().to_primitive(),
             f64(theta).cos().to_primitive(),
             sin_theta * f64(phi).sin().to_primitive() };
}
auto face_uv(Direction d, unsigned size) -> FaceUv {
    double   ax = f64(d.x).abs().to_primitive(), ay = f64(d.y).abs().to_primitive();
    double   az = f64(d.z).abs().to_primitive(), major {}, sc {}, tc {};
    unsigned face {};
    if (ax >= ay && ax >= az) {
        major = ax;
        face  = d.x >= 0 ? 0 : 1;
        sc    = d.x >= 0 ? -d.z : d.z;
        tc    = -d.y;
    } else if (ay >= az) {
        major = ay;
        face  = d.y >= 0 ? 2 : 3;
        sc    = d.x;
        tc    = d.y >= 0 ? d.z : -d.z;
    } else {
        major = az;
        face  = d.z >= 0 ? 4 : 5;
        sc    = d.z >= 0 ? d.x : -d.x;
        tc    = -d.y;
    }
    auto extent  = double(size - 1) / 2;
    auto bounded = [size](double coordinate) {
        return rstd::cmp::max(0.0, rstd::cmp::min(coordinate, double(size - 1)));
    };
    return { face, bounded((sc / major + 1) * extent), bounded((tc / major + 1) * extent) };
}
auto logluv(array<double, 4> value) -> Rgb {
    double y  = f64((value[usize(2)] * 255 + value[usize(3)] - 127) / 2).exp2().to_primitive();
    double zp = value[usize(1)] == 0 ? 0 : y / value[usize(1)];
    double xp = value[usize(0)] * zp;
    return { rstd::cmp::max(0.0, 6.0013 * xp - 1.3320 * y + 0.3007 * zp),
             rstd::cmp::max(0.0, -2.7000 * xp + 3.1029 * y - 1.0880 * zp),
             rstd::cmp::max(0.0, -1.7995 * xp - 5.7720 * y + 5.6268 * zp) };
}
auto cube_pixel(slice<u8> bytes, unsigned size, unsigned offset, Direction d) -> Rgb {
    auto             uv = face_uv(d, size);
    auto             x0 = unsigned(f64(uv.u).floor().to_primitive()),
                     y0 = unsigned(f64(uv.v).floor().to_primitive());
    auto             x1 = rstd::cmp::min(x0 + 1, size - 1), y1 = rstd::cmp::min(y0 + 1, size - 1);
    double           fx = uv.u - x0, fy = uv.v - y0;
    array<double, 4> encoded;
    for (unsigned channel = 0; channel < 4; ++channel) {
        unsigned base = offset + (uv.face * 4 + channel) * size * size;
        auto     at   = [&](unsigned x, unsigned y) {
            return double(bytes[usize(base + y * size + x)].to_primitive()) / 255;
        };
        auto a                  = at(x0, y0) * (1 - fx) + at(x1, y0) * fx;
        auto b                  = at(x0, y1) * (1 - fx) + at(x1, y1) * fx;
        encoded[usize(channel)] = a * (1 - fy) + b * fy;
    }
    return logluv(encoded);
}
auto diffuse_pixel(const array<f64, 27>& sh, Direction d) -> Rgb {
    auto root = [](double value) {
        return f64(value).sqrt().to_primitive();
    };
    array<double, 9> scale { 1 / (2 * root(pi)),  -0.5 * root(3 / pi),  0.5 * root(3 / pi),
                             -0.5 * root(3 / pi), 0.5 * root(15 / pi),  -0.5 * root(15 / pi),
                             0.25 * root(5 / pi), -0.5 * root(15 / pi), 0.25 * root(15 / pi) };
    array<double, 9> basis { 1.0,
                             d.y,
                             d.z,
                             d.x,
                             d.y * d.x,
                             d.y * d.z,
                             3 * d.z * d.z - 1,
                             d.z * d.x,
                             d.x * d.x - d.y * d.y };
    array<double, 3> rgb {};
    for (usize i {}; i < usize(9); ++i)
        for (usize c {}; c < usize(3); ++c)
            rgb[c] += sh[i * usize(3) + c].to_primitive() * scale[i] * basis[i];
    return { rstd::cmp::max(rgb[usize(0)], 0.0),
             rstd::cmp::max(rgb[usize(1)], 0.0),
             rstd::cmp::max(rgb[usize(2)], 0.0) };
}
auto brdf_pixel(slice<u8> bytes, unsigned index) -> Rgb {
    auto at = [&](unsigned channel) {
        return unsigned(bytes[usize(index * 4 + channel)].to_primitive());
    };
    return { double(at(1) * 256 + at(0)) / 65535, double(at(3) * 256 + at(2)) / 65535, 0 };
}
auto rgbe(Rgb rgb) -> Prepared<array<u8, 4>> {
    auto maximum = rstd::cmp::max(rgb.r, rstd::cmp::max(rgb.g, rgb.b));
    if (! f64(rgb.r).is_finite() || ! f64(rgb.g).is_finite() || ! f64(rgb.b).is_finite() ||
        rgb.r < 0 || rgb.g < 0 || rgb.b < 0 || maximum >= 1e38)
        return Err(fail("HDR pixel is outside finite RGBE range"_str));
    array<u8, 4> pixel {};
    if (maximum <= 1e-32) return Ok(rstd::move(pixel));
    auto exponent   = int(f64(maximum).log2().floor().to_primitive()) + 1;
    auto scale      = f64(1).ldexp(i32(8 - exponent)).to_primitive();
    pixel[usize(0)] = u8(unsigned(rstd::cmp::min(rgb.r * scale, 255.0)));
    pixel[usize(1)] = u8(unsigned(rstd::cmp::min(rgb.g * scale, 255.0)));
    pixel[usize(2)] = u8(unsigned(rstd::cmp::min(rgb.b * scale, 255.0)));
    pixel[usize(3)] = u8(unsigned(exponent + 128));
    return Ok(rstd::move(pixel));
}

void encode_row(slice<u8> row, unsigned width, Vec<u8>& encoded) {
    encoded.clear();
    if (width < 8) {
        encoded.extend_from_slice(row);
        return;
    }
    encoded.push(u8(2));
    encoded.push(u8(2));
    encoded.push(u8(width >> 8));
    encoded.push(u8(width & 255));
    for (unsigned c = 0; c < 4; ++c)
        for (unsigned x = 0; x < width;) {
            auto n = rstd::cmp::min(128u, width - x);
            encoded.push(u8(n));
            for (unsigned i = 0; i < n; ++i) encoded.push(row[usize((x + i) * 4 + c)]);
            x += n;
        }
}

struct Output {
    PathBuf   root;
    JsonArray files;
    unsigned  total {};

    auto write(rstd::fs::File& file, slice<u8> bytes, unsigned& count) -> Prepared<empty> {
        if (bytes.len() > usize(max_output - total))
            return Err(fail("output byte limit exceeded"_str));
        auto result = file.write_all(bytes);
        if (result.is_err()) return Err(rstd::format("HDR write: {}", result.unwrap_err()));
        auto added = unsigned(bytes.len().to_primitive());
        count += added;
        total += added;
        return Ok(empty {});
    }

    template<class Pixel>
    auto hdr(ref<str> prefix, ref<str> name, unsigned width, unsigned height, Pixel sample)
        -> Prepared<String> {
        if (files.len() >= usize(64)) return Err(fail("output file limit exceeded"_str));
        auto relative = rstd::format("{}/{}", prefix, name);
        auto path     = root.join(PathBuf::from(relative.as_str()).as_path());
        auto opened   = rstd::fs::File::create_new(path.as_path());
        if (opened.is_err()) return Err(rstd::format("HDR create_new: {}", opened.unwrap_err()));
        auto file = opened.unwrap();
        auto header =
            rstd::format("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {} +X {}\n", height, width);
        unsigned count {};
        auto     written = write(file, header->as_bytes(), count);
        if (written.is_err()) return Err(written.unwrap_err());
        Vec<u8> row;
        row.resize(usize(width * 4), u8(0));
        Vec<u8> encoded;
        encoded.reserve(usize(width * 4 + 4 * (width / 128 + 1) + 4));
        for (unsigned y = 0; y < height; ++y) {
            for (unsigned x = 0; x < width; ++x) {
                auto pixel = rgbe(sample(x, y));
                if (pixel.is_err()) return Err(pixel.unwrap_err());
                auto value = pixel.unwrap();
                for (unsigned c = 0; c < 4; ++c) row[usize(x * 4 + c)] = value[usize(c)];
            }
            encode_row(row.as_slice(), width, encoded);
            written = write(file, encoded.as_slice(), count);
            if (written.is_err()) return Err(written.unwrap_err());
        }
        JsonMap entry;
        put(entry, "path"_str, jstring(relative.as_str()));
        put(entry, "bytes"_str, jnumber(count));
        files.push(Json::Object(rstd::move(entry)));
        return Ok(rstd::move(relative));
    }
};

auto prepare(Request request, PathBuf input_root, PathBuf output_root) -> Prepared<Json> {
    auto prefix_dir = output_root.join(PathBuf::from(request.prefix.as_str()).as_path());
    auto created    = rstd::fs::create_dir(prefix_dir.as_path());
    if (created.is_err()) return Err(rstd::format("output create_dir: {}", created.unwrap_err()));
    Output  output { rstd::move(output_root), {}, 0 };
    JsonMap preprocessed;
    if (request.background) {
        auto& config = *request.background;
        auto  loaded = load_resource(
            request, input_root, config.resource.as_str(), config.face * config.face * 24);
        if (loaded.is_err()) return Err(loaded.unwrap_err());
        auto bytes = loaded.unwrap();
        auto width = config.face * 4, height = config.face * 2;
        auto path = output.hdr(
            request.prefix.as_str(),
            "background.hdr"_str,
            width,
            height,
            [&](unsigned x, unsigned y) {
                return cube_pixel(bytes.as_slice(), config.face, 0, direction(x, y, width, height));
            });
        if (path.is_err()) return Err(path.unwrap_err());
        put(preprocessed, "background"_str, jstring(path.unwrap().as_str()));
    }
    if (request.sh) {
        auto path = output.hdr(
            request.prefix.as_str(), "diffuse.hdr"_str, 128, 64, [&](unsigned x, unsigned y) {
                return diffuse_pixel(*request.sh, direction(x, y, 128, 64));
            });
        if (path.is_err()) return Err(path.unwrap_err());
        put(preprocessed, "diffuse"_str, jstring(path.unwrap().as_str()));
    }
    if (request.brdf) {
        auto& config = *request.brdf;
        auto  loaded = load_resource(
            request, input_root, config.resource.as_str(), config.width * config.height * 4);
        if (loaded.is_err()) return Err(loaded.unwrap_err());
        auto bytes = loaded.unwrap();
        auto path  = output.hdr(request.prefix.as_str(),
                                "brdf.hdr"_str,
                                config.width,
                                config.height,
                                [&](unsigned x, unsigned y) {
                                   return brdf_pixel(bytes.as_slice(), y * config.width + x);
                                });
        if (path.is_err()) return Err(path.unwrap_err());
        put(preprocessed, "brdf"_str, jstring(path.unwrap().as_str()));
        put(preprocessed, "brdf_filter"_str, jstring("nearest"_str));
    }
    if (request.specular) {
        auto&    config = *request.specular;
        unsigned expected {}, levels {}, offset {};
        for (unsigned size = config.face; size; size /= 2) {
            expected += size * size * 24;
            ++levels;
        }
        auto loaded = load_resource(request, input_root, config.resource.as_str(), expected);
        if (loaded.is_err()) return Err(loaded.unwrap_err());
        auto      bytes = loaded.unwrap();
        JsonArray paths;
        unsigned  size = config.face;
        for (unsigned level = 0; level < levels; ++level) {
            auto width = size * 4, height = size * 2;
            auto name = rstd::format("specular-{}.hdr", level);
            auto path = output.hdr(
                request.prefix.as_str(), name.as_str(), width, height, [&](unsigned x, unsigned y) {
                    return cube_pixel(
                        bytes.as_slice(), size, offset, direction(x, y, width, height));
                });
            if (path.is_err()) return Err(path.unwrap_err());
            paths.push(jstring(path.unwrap().as_str()));
            offset += size * size * 24;
            size /= 2;
        }
        unsigned scale {};
        for (unsigned size = config.face / config.limit; size > 1; size /= 2) ++scale;
        put(preprocessed, "specular"_str, Json::Array(rstd::move(paths)));
        put(preprocessed, "specular_lod_scale"_str, jnumber(scale));
        put(preprocessed, "specular_lod_exponent"_str, jfloat(0.5));
    }
    JsonMap data;
    put(data, "preprocessed"_str, Json::Object(rstd::move(preprocessed)));
    JsonMap response;
    put(response, "version"_str, jnumber(1));
    put(response, "files"_str, Json::Array(rstd::move(output.files)));
    put(response, "data"_str, Json::Object(rstd::move(data)));
    return Ok(Json::Object(rstd::move(response)));
}

auto directory(ref<str> text) -> Prepared<PathBuf> {
    auto result = rstd::fs::canonicalize(PathBuf::from(text).as_path());
    if (result.is_err()) return Err(rstd::format("root canonicalize: {}", result.unwrap_err()));
    auto path     = result.unwrap();
    auto metadata = rstd::fs::metadata(path.as_path());
    if (metadata.is_err()) return Err(rstd::format("root metadata: {}", metadata.unwrap_err()));
    if (! metadata.unwrap().is_dir()) return Err(fail("root must be a directory"_str));
    return Ok(rstd::move(path));
}
auto execute(ref<str> request_path, ref<str> input_path, ref<str> output_path) -> Prepared<Json> {
    auto input = directory(input_path);
    if (input.is_err()) return Err(input.unwrap_err());
    auto output = directory(output_path);
    if (output.is_err()) return Err(output.unwrap_err());
    auto source = read_file(PathBuf::from(request_path), 65536);
    if (source.is_err()) return Err(source.unwrap_err());
    auto parsed = rstd::json::from_slice(
        source.unwrap().as_slice(),
        { .reject_duplicate_keys = true, .reject_integer_overflow = true, .max_depth = usize(12) });
    if (parsed.is_err()) return Err(rstd::format("request JSON: {}", parsed.unwrap_err()));
    auto request = parse_request(parsed.unwrap());
    if (request.is_err()) return Err(request.unwrap_err());
    return prepare(request.unwrap(), input.unwrap(), output.unwrap());
}

auto selftest() -> Prepared<empty> {
    auto close = [](double a, double b) {
        return f64(a - b).abs() < f64(1e-10);
    };
    array<Direction, 6> axes { Direction { 1, 0, 0 }, Direction { -1, 0, 0 },
                               Direction { 0, 1, 0 }, Direction { 0, -1, 0 },
                               Direction { 0, 0, 1 }, Direction { 0, 0, -1 } };
    Vec<u8>             raw;
    raw.resize(usize(24 * 4), u8(0));
    for (unsigned face = 0; face < 6; ++face)
        for (unsigned texel = 0; texel < 4; ++texel) {
            raw[usize(face * 16 + texel)]     = u8(80 + face);
            raw[usize(face * 16 + 4 + texel)] = u8(180);
            raw[usize(face * 16 + 8 + texel)] = u8(127);
        }
    for (usize i {}; i < usize(6); ++i) {
        auto uv       = face_uv(axes[i], 2);
        auto actual   = cube_pixel(raw.as_slice(), 2, 0, axes[i]);
        auto expected = logluv(array<double, 4> {
            (80 + double(i.to_primitive())) / 255, 180.0 / 255, 127.0 / 255, 0.0 });
        if (uv.face != i.to_primitive() || ! close(uv.u, 0.5) || ! close(uv.v, 0.5) ||
            ! close(actual.r, expected.r) || ! close(actual.g, expected.g) ||
            ! close(actual.b, expected.b))
            return Err(fail("selftest cube axes/planar channels"_str));
    }
    if (face_uv({ 1, 0.5, -0.5 }, 3).u != 1.5 || face_uv({ 0.5, 1, 0.5 }, 3).v != 1.5 ||
        face_uv({ 0.5, -1, 0.5 }, 3).v != 0.5 || face_uv({ 0.5, 0.5, -1 }, 3).u != 0.5)
        return Err(fail("selftest cube orientation"_str));
    raw[usize(9)]     = u8(129);
    raw[usize(11)]    = u8(129);
    auto interpolated = cube_pixel(raw.as_slice(), 2, 0, { 1, 0, 0 });
    auto midpoint     = logluv(array<double, 4> { 80.0 / 255, 180.0 / 255, 128.0 / 255, 0.0 });
    if (! close(interpolated.r, midpoint.r) || ! close(interpolated.g, midpoint.g) ||
        ! close(interpolated.b, midpoint.b))
        return Err(fail("selftest interpolate encoded LogLuv before decoding"_str));
    array<f64, 27> sh {};
    sh[usize(0)] = f64(2 * f64(pi).sqrt().to_primitive());
    sh[usize(1)] = sh[usize(0)] * f64(2);
    sh[usize(2)] = sh[usize(0)] * f64(3);
    auto diffuse = diffuse_pixel(sh, { 0, 1, 0 });
    if (! close(diffuse.r, 1) || ! close(diffuse.g, 2) || ! close(diffuse.b, 3))
        return Err(fail("selftest diffuse SH normalization"_str));
    array<u8, 4> lut { u8(0x34), u8(0x12), u8(0xcd), u8(0xab) };
    auto         brdf = brdf_pixel(lut.as_slice(), 0);
    if (! close(brdf.r, 4660.0 / 65535) || ! close(brdf.g, 43981.0 / 65535) || brdf.b != 0)
        return Err(fail("selftest BRDF channel packing"_str));
    auto encoded = rgbe({ 1, 0.5, 0.25 });
    if (encoded.is_err()) return Err(encoded.unwrap_err());
    auto pixel = encoded.unwrap();
    if (pixel != array<u8, 4> { u8(128), u8(64), u8(32), u8(129) })
        return Err(fail("selftest RGBE exponent"_str));
    Vec<u8> row;
    for (unsigned i = 0; i < 4; ++i) row.extend_from_slice(pixel.as_slice());
    Vec<u8> scanline;
    encode_row(row.as_slice(), 4, scanline);
    if (scanline.as_slice() != row.as_slice())
        return Err(fail("selftest small HDR must be flat RGBE"_str));
    row.extend_from_slice(row.clone().as_slice());
    encode_row(row.as_slice(), 8, scanline);
    if (scanline.len() != usize(40) || scanline[usize(3)] != u8(8) || scanline[usize(4)] != u8(8))
        return Err(fail("selftest HDR scanline literal packets"_str));
    auto compressed = licompto::compress<licompto::GzipEncoder>(raw.as_slice());
    if (compressed.is_err())
        return Err(rstd::format("selftest gzip compress: {}", compressed.unwrap_err()));
    auto gzip    = compressed.unwrap();
    auto decoded = decode_bytes(gzip.clone(), 96);
    if (decoded.is_err() || decoded.unwrap().as_slice() != raw.as_slice())
        return Err(fail("selftest gzip roundtrip"_str));
    if (decode_bytes(gzip.clone(), 95).is_ok() || decode_bytes(raw.clone(), 95).is_ok())
        return Err(fail("selftest exact resource budget"_str));
    auto corrupt                      = gzip.clone();
    corrupt[corrupt.len() - usize(8)] = corrupt[corrupt.len() - usize(8)] ^ u8(1);
    if (decode_bytes(rstd::move(corrupt), 96).is_ok())
        return Err(fail("selftest gzip checksum"_str));
    auto truncated = gzip.clone();
    truncated.pop();
    if (decode_bytes(rstd::move(truncated), 96).is_ok())
        return Err(fail("selftest gzip truncation"_str));
    gzip.push(u8(0));
    if (decode_bytes(rstd::move(gzip), 96).is_ok())
        return Err(fail("selftest gzip trailing bytes"_str));
    if (normal_relative("../input"_str) || normal_relative("/input"_str) ||
        normal_relative("./input"_str) || normal_relative("a/./b"_str) ||
        normal_relative("a//b"_str) || normal_relative("a/"_str) || normal_relative("a\\b"_str) ||
        safe_prefix("../output"_str) || ! normal_relative("a/b.bin"_str))
        return Err(fail("selftest relative paths"_str));
    return Ok(empty {});
}
} // namespace

int run(int argc, char** argv) {
    if (argc == 2 &&
        rstd::ffi::CStr::from_ptr(argv[1]).to_str().unwrap_or(""_str) == "--selftest"_str) {
        auto result = selftest();
        if (result.is_err()) {
            rstd::io::eprintln("{}", result.unwrap_err());
            return 1;
        }
        rstd::io::eprintln("sketchfab prepare selftest passed");
        return 0;
    }
    String request, input, output;
    for (int i = 1; i < argc; ++i) {
        auto key = rstd::ffi::CStr::from_ptr(argv[i]).to_str();
        if (key.is_err() || i + 1 >= argc) {
            rstd::io::eprintln("expected --request FILE --input-root DIR --output-root DIR");
            return 2;
        }
        auto value = rstd::ffi::CStr::from_ptr(argv[++i]).to_str();
        if (value.is_err()) {
            rstd::io::eprintln("argument must be UTF-8");
            return 2;
        }
        auto    target = key.unwrap();
        String* field  = target == "--request"_str       ? &request
                         : target == "--input-root"_str  ? &input
                         : target == "--output-root"_str ? &output
                                                         : nullptr;
        if (! field || ! field->is_empty()) {
            rstd::io::eprintln("unknown or repeated argument");
            return 2;
        }
        *field = String::make(value.unwrap());
    }
    if (request.is_empty() || input.is_empty() || output.is_empty()) {
        rstd::io::eprintln("expected --request FILE --input-root DIR --output-root DIR");
        return 2;
    }
    auto result = execute(request.as_str(), input.as_str(), output.as_str());
    if (result.is_err()) {
        rstd::io::eprintln("sketchfab prepare: {}", result.unwrap_err());
        return 1;
    }
    auto json = rstd::json::to_string(result.unwrap());
    if (json.len() > usize(65536)) {
        rstd::io::eprintln("response byte limit exceeded");
        return 1;
    }
    rstd::io::println("{}", json);
    return 0;
}
} // namespace waywallen::sketchfab
