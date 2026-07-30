-- Exercises the extended string library: rep, format, find, match, gmatch, gsub.

-- string.rep
assert(string.rep("ab", 3) == "ababab")
assert(string.rep("ab", 0) == "")
assert(string.rep("ab", 3, ",") == "ab,ab,ab")
assert(string.rep("x", 1) == "x")

-- string.format: integers
assert(string.format("%d", 42) == "42")
assert(string.format("%d", -7) == "-7")
assert(string.format("%5d", 42) == "   42")
assert(string.format("%-5d|", 42) == "42   |")
assert(string.format("%05d", 42) == "00042")
assert(string.format("%+d", 42) == "+42")
assert(string.format("%x", 255) == "ff")
assert(string.format("%X", 255) == "FF")
assert(string.format("%#x", 255) == "0xff")
assert(string.format("%o", 8) == "10")

-- string.format: char and percent
assert(string.format("%c", 65) == "A")
assert(string.format("100%%") == "100%")

-- string.format: floats
assert(string.format("%.2f", 3.14159) == "3.14")
assert(string.format("%f", 3.5) == "3.500000")
assert(string.format("%10.2f", 3.5) == "      3.50")

-- string.format: strings and quoting
assert(string.format("%s", "hello") == "hello")
assert(string.format("%.3s", "hello") == "hel")
assert(string.sub(string.format("%q", 'a"b'), 1, 1) == '"')

-- string.format: multiple args
assert(string.format("%s=%d", "x", 5) == "x=5")

-- string.find (plain)
local s, e = string.find("hello world", "world", 1, true)
assert(s == 7 and e == 11)
assert(string.find("hello", "xyz", 1, true) == nil)

-- string.find (pattern) with captures
s, e = string.find("hello", "ell")
assert(s == 2 and e == 4)
local a, b = string.find("key=value", "(%w+)=(%w+)")
assert(a == 1 and b == 9)

-- string.match
assert(string.match("hello123", "%d+") == "123")
local k, v = string.match("name=bob", "(%w+)=(%w+)")
assert(k == "name" and v == "bob")
assert(string.match("abc", "%d+") == nil)

-- string.gmatch
local words = {}
for w in string.gmatch("one two three", "%a+") do
  words[#words + 1] = w
end
assert(#words == 3)
assert(words[1] == "one" and words[2] == "two" and words[3] == "three")

-- string.gmatch with captures
local pairs = {}
for k, v in string.gmatch("a=1,b=2", "(%w+)=(%w+)") do
  pairs[k] = v
end
assert(pairs["a"] == "1" and pairs["b"] == "2")

-- string.gsub with string replacement
local r, n = string.gsub("hello world", "o", "0")
assert(r == "hell0 w0rld" and n == 2)

-- string.gsub with capture references in replacement
r, n = string.gsub("John Smith", "(%w+) (%w+)", "%2 %1")
assert(r == "Smith John" and n == 1)

-- string.gsub with table replacement (count includes nil-replaced matches)
r, n = string.gsub("a and b", "%a+", { a = "X", b = "Y" })
assert(r == "X and Y" and n == 3)

-- string.gsub with function replacement
r, n = string.gsub("abc", "%a", function(c) return string.upper(c) end)
assert(r == "ABC" and n == 3)

-- string.gsub with limit
r, n = string.gsub("aaaa", "a", "b", 2)
assert(r == "bbaa" and n == 2)

-- string.gsub keeps original when replacement is nil/false
r, n = string.gsub("abc", "%a", function(c)
  if c == "b" then return nil end
  return c
end)
assert(r == "abc")

print("string_ext ok")
