// SPDX-License-Identifier:BSL-1.0
//
// Reference digests for tests/cpp_differential.rs: C++ Boost.Polygon's segment Voronoi
// diagram of each input (segments only), digested the same way the Rust test digests
// boostvoronoi's.
// Regenerate tests/data/cpp_reference_digests.txt with (header-only Boost, g++/libstdc++):
//
//   g++ -O2 -std=c++17 -I <boost_1_86_0> boost_digest.cpp -o boost_digest
//   ./boost_digest ../data/part28_wall_segments.txt > ../data/cpp_reference_digests.txt
//
// The fuzz generator must stay identical to `fuzz_case` in the Rust test.
#include <boost/polygon/voronoi.hpp>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <vector>
using namespace boost::polygon;
struct Pt { int x, y; };
struct Seg { Pt p0, p1; };
namespace boost { namespace polygon {
template <> struct geometry_concept<Pt> { typedef point_concept type; };
template <> struct point_traits<Pt> {
  typedef int coordinate_type;
  static int get(const Pt& p, orientation_2d o) { return o == HORIZONTAL ? p.x : p.y; }
};
template <> struct geometry_concept<Seg> { typedef segment_concept type; };
template <> struct segment_traits<Seg> {
  typedef int coordinate_type; typedef Pt point_type;
  static Pt get(const Seg& s, direction_1d d) { return d.to_int() ? s.p1 : s.p0; }
};
}}

struct Fnv { uint64_t h = 0xcbf29ce484222325ull;
  void word(uint64_t w) { for (int i = 0; i < 8; ++i) { h ^= (w >> (8 * i)) & 0xff; h *= 0x100000001b3ull; } } };
static uint64_t bits(double d) { uint64_t u; std::memcpy(&u, &d, 8); return u; }

static void digest(const char* name, const std::vector<Seg>& segs) {
  voronoi_diagram<double> vd; construct_voronoi(segs.begin(), segs.end(), &vd);
  const auto* c0 = &vd.cells()[0]; const auto* v0 = vd.vertices().empty() ? nullptr : &vd.vertices()[0];
  const auto* e0 = &vd.edges()[0];
  Fnv f; f.word(vd.num_cells()); f.word(vd.num_vertices()); f.word(vd.num_edges());
  // Each cell by the site it stands for (the libraries number sources differently): a point
  // (1, x, y) or a segment (3, x0, y0, x1, y1) of the input.
  auto i = [](int v) { return (uint64_t)(int64_t)v; };
  for (const auto& c : vd.cells()) {
    const Seg& g = segs[c.source_index()];
    switch (c.source_category()) {
      case SOURCE_CATEGORY_SEGMENT_START_POINT: f.word(1); f.word(i(g.p0.x)); f.word(i(g.p0.y)); break;
      case SOURCE_CATEGORY_SEGMENT_END_POINT: f.word(1); f.word(i(g.p1.x)); f.word(i(g.p1.y)); break;
      default: f.word(3); f.word(i(g.p0.x)); f.word(i(g.p0.y)); f.word(i(g.p1.x)); f.word(i(g.p1.y));
    }
  }
  for (const auto& v : vd.vertices()) { f.word(bits(v.x())); f.word(bits(v.y())); }
  for (const auto& e : vd.edges()) {
    f.word(e.cell() - c0);
    f.word(e.vertex0() ? (uint64_t)(e.vertex0() - v0) : ~0ull);
    f.word(e.twin() - e0); f.word(e.next() - e0); f.word(e.prev() - e0);
    f.word(e.is_primary()); f.word(e.is_linear());
  }
  printf("%s %zu %zu %zu %016llx\n", name, vd.num_cells(), vd.num_vertices(), vd.num_edges(), (unsigned long long)f.h);
}

struct Lcg { uint64_t s; uint32_t next() { s = s * 6364136223846793005ull + 1442695040888963407ull; return (uint32_t)(s >> 33); }
  int range(int lo, int hi) { return lo + (int)(next() % (uint32_t)(hi - lo + 1)); } };

// Rectangles on a grid (some left out), some with a cut corner and some with an extra vertex on
// their left side: many vertical and collinear segments, none crossing.
static std::vector<Seg> fuzz_case(uint64_t seed) {
  Lcg r{seed * 0x9e3779b97f4a7c15ull + 1};
  std::vector<Seg> segs;
  int n = r.range(2, 9), m = r.range(2, 9);
  const int widths[3] = {1000, 100000, 50000000};
  int w = widths[r.range(0, 2)];
  for (int i = 0; i < n; ++i) for (int j = 0; j < m; ++j) {
    if (r.range(0, 9) < 3) continue;
    int x0 = i * w + r.range(1, w / 4), x1 = (i + 1) * w - r.range(1, w / 4);
    int y0 = j * w + r.range(1, w / 4), y1 = (j + 1) * w - r.range(1, w / 4);
    std::vector<Pt> pts = {{x0, y0}, {x1, y0}, {x1, y1}, {x0, y1}};
    if (r.range(0, 1)) { int c = r.range(1, (x1 - x0) / 3 > 1 ? (x1 - x0) / 3 : 1); pts = {{x0, y0}, {x1 - c, y0}, {x1, y0 + c}, {x1, y1}, {x0, y1}}; }
    if (r.range(0, 9) < 3) { int k = r.range(1, (y1 - y0) / 3 > 1 ? (y1 - y0) / 3 : 1); pts.push_back({x0, y1 - k}); }
    for (size_t a = 0; a < pts.size(); ++a) { Pt p = pts[a], q = pts[(a + 1) % pts.size()]; if (p.x != q.x || p.y != q.y) segs.push_back({p, q}); }
  }
  return segs;
}

int main(int argc, char** argv) {
  std::ifstream in(argv[1]); std::vector<Seg> part; Seg s;
  while (in >> s.p0.x >> s.p0.y >> s.p1.x >> s.p1.y) part.push_back(s);
  digest("part28", part);
  for (int i = 0; i < 400; ++i) { char name[32]; snprintf(name, sizeof name, "case%d", i); digest(name, fuzz_case(i)); }
}
