-- Comprehensive coverage of the table library: concat, insert, move, pack,
-- remove, sort, unpack.

-- table.concat
assert(table.concat({ "a", "b", "c" }) == "abc")
assert(table.concat({ "a", "b", "c" }, "-") == "a-b-c")
assert(table.concat({ "a", "b", "c" }, ",", 2, 3) == "b,c")
assert(table.concat({}) == "")
assert(table.concat({ 1, 2, 3 }, "") == "123")

-- table.insert (append and positional)
local t = { 1, 2, 3 }
table.insert(t, 4)
assert(t[4] == 4 and #t == 4)
table.insert(t, 1, 99)
assert(t[1] == 99 and t[2] == 1 and #t == 5)

-- table.remove (from position and from end)
local removed = table.remove(t, 1)
assert(removed == 99 and t[1] == 1 and #t == 4)
local last = table.remove(t)
assert(last == 4 and #t == 3)

-- table.move
local src = { 1, 2, 3, 4, 5 }
local dst = {}
table.move(src, 2, 4, 1, dst)
assert(dst[1] == 2 and dst[2] == 3 and dst[3] == 4)
-- move within the same table
local m = { 1, 2, 3, 4, 5 }
table.move(m, 1, 3, 2)
assert(m[2] == 1 and m[3] == 2 and m[4] == 3)

-- table.pack
local p = table.pack(10, 20, 30)
assert(p.n == 3 and p[1] == 10 and p[2] == 20 and p[3] == 30)
local empty = table.pack()
assert(empty.n == 0)

-- table.unpack
local a, b, c = table.unpack({ 7, 8, 9 })
assert(a == 7 and b == 8 and c == 9)
local x, y = table.unpack({ 1, 2, 3, 4 }, 2, 3)
assert(x == 2 and y == 3)

-- table.sort (default and custom comparator)
local s = { 3, 1, 2 }
table.sort(s)
assert(s[1] == 1 and s[2] == 2 and s[3] == 3)
table.sort(s, function(a, b) return a > b end)
assert(s[1] == 3 and s[2] == 2 and s[3] == 1)

-- table.sort on strings
local words = { "banana", "apple", "cherry" }
table.sort(words)
assert(words[1] == "apple" and words[2] == "banana" and words[3] == "cherry")

print("table_ext ok")
