-- Exercises the utf8 library: char, codepoint, len, offset, charpattern.

-- utf8.char builds a UTF-8 string from codepoints.
local s = utf8.char(0x48, 0xE9, 0x1F600) -- "H", "é", "😀"
assert(utf8.len(s) == 3)

-- utf8.codepoint returns the codepoints (i, j are byte positions).
local a, b, c = utf8.codepoint(s, 1, -1)
assert(a == 0x48)
assert(b == 0xE9)
assert(c == 0x1F600)
-- Byte range 1..3 covers only the first two codepoints (H at byte 1, é at byte 2).
local x, y, z = utf8.codepoint(s, 1, 3)
assert(x == 0x48 and y == 0xE9 and z == nil)

-- utf8.len counts codepoints, not bytes.
assert(utf8.len("hello") == 5)
assert(#s == 7) -- byte length: H=1, é=2, 😀=4 => 7
assert(utf8.len(s) == 3)

-- utf8.offset converts codepoint indices to byte positions.
assert(utf8.offset(s, 1) == 1)
assert(utf8.offset(s, 2) == 2) -- second codepoint starts at byte 2
assert(utf8.offset(s, 3) == 4) -- third codepoint starts at byte 4

-- utf8.charpattern matches one codepoint at a time.
local count = 0
for _ in string.gmatch(s, utf8.charpattern) do
  count = count + 1
end
assert(count == 3)

-- Round trip: codepoint(char(x)) == x
for _, cp in ipairs({ 65, 233, 0x4E2D, 0x1F600 }) do
  assert(utf8.codepoint(utf8.char(cp)) == cp)
end

print("utf8 ok")
