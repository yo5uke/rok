# A small JSON parser in base R, for the structured results of the rok binary
# (requirements chapter 4: the package depends on base R only).
#
# Objects become named lists, arrays become lists (or atomic vectors when every
# element is a single value of the same type), and null becomes NULL.

parse_json <- function(text) {
  chars <- strsplit(enc2utf8(text), "", fixed = TRUE)[[1L]]
  n <- length(chars)
  pos <- 1L

  fail <- function(what) {
    stop(sprintf("Invalid JSON at character %d: %s.", pos, what), call. = FALSE)
  }
  skip <- function() {
    while (pos <= n && chars[[pos]] %in% c(" ", "\t", "\n", "\r")) pos <<- pos + 1L
  }
  expect <- function(word) {
    len <- nchar(word)
    if (pos + len - 1L > n ||
        paste(chars[pos:(pos + len - 1L)], collapse = "") != word) {
      fail(sprintf("expected `%s`", word))
    }
    pos <<- pos + len
  }

  value <- function() {
    skip()
    if (pos > n) fail("unexpected end")
    switch(chars[[pos]],
      "{" = object(),
      "[" = array(),
      "\"" = string(),
      "t" = { expect("true"); TRUE },
      "f" = { expect("false"); FALSE },
      "n" = { expect("null"); NULL },
      number()
    )
  }

  object <- function() {
    pos <<- pos + 1L
    out <- list()
    skip()
    if (pos <= n && chars[[pos]] == "}") {
      pos <<- pos + 1L
      return(structure(list(), names = character()))
    }
    repeat {
      skip()
      if (pos > n || chars[[pos]] != "\"") fail("expected a key")
      key <- string()
      skip()
      if (pos > n || chars[[pos]] != ":") fail("expected `:`")
      pos <<- pos + 1L
      out[key] <- list(value())
      skip()
      if (pos > n) fail("unexpected end")
      if (chars[[pos]] == ",") {
        pos <<- pos + 1L
      } else if (chars[[pos]] == "}") {
        pos <<- pos + 1L
        return(out)
      } else {
        fail("expected `,` or `}`")
      }
    }
  }

  array <- function() {
    pos <<- pos + 1L
    out <- list()
    skip()
    if (pos <= n && chars[[pos]] == "]") {
      pos <<- pos + 1L
      return(out)
    }
    repeat {
      out[length(out) + 1L] <- list(value())
      skip()
      if (pos > n) fail("unexpected end")
      if (chars[[pos]] == ",") {
        pos <<- pos + 1L
      } else if (chars[[pos]] == "]") {
        pos <<- pos + 1L
        return(simplify(out))
      } else {
        fail("expected `,` or `]`")
      }
    }
  }

  string <- function() {
    pos <<- pos + 1L
    parts <- character()
    start <- pos
    repeat {
      if (pos > n) fail("unterminated string")
      ch <- chars[[pos]]
      if (ch == "\"") {
        if (pos > start) parts <- c(parts, chars[start:(pos - 1L)])
        pos <<- pos + 1L
        return(paste(parts, collapse = ""))
      }
      if (ch != "\\") {
        pos <<- pos + 1L
        next
      }
      if (pos > start) parts <- c(parts, chars[start:(pos - 1L)])
      if (pos + 1L > n) fail("unterminated escape")
      esc <- chars[[pos + 1L]]
      pos <<- pos + 2L
      if (esc == "u") {
        code <- hex4()
        # A surrogate pair encodes a character outside the basic plane.
        if (code >= 0xD800 && code <= 0xDBFF && pos + 1L <= n &&
            chars[[pos]] == "\\" && chars[[pos + 1L]] == "u") {
          pos <<- pos + 2L
          low <- hex4()
          code <- 0x10000 + (code - 0xD800) * 0x400 + (low - 0xDC00)
        }
        parts <- c(parts, intToUtf8(code))
      } else {
        parts <- c(parts, switch(esc,
          "\"" = "\"", "\\" = "\\", "/" = "/", "b" = "\b", "f" = "\f",
          "n" = "\n", "r" = "\r", "t" = "\t",
          fail(sprintf("invalid escape `\\%s`", esc))
        ))
      }
      start <- pos
    }
  }

  hex4 <- function() {
    if (pos + 3L > n) fail("short \\u escape")
    code <- strtoi(paste(chars[pos:(pos + 3L)], collapse = ""), 16L)
    if (is.na(code)) fail("invalid \\u escape")
    pos <<- pos + 4L
    code
  }

  number <- function() {
    start <- pos
    while (pos <= n && chars[[pos]] %in% c("-", "+", ".", "e", "E", 0:9)) {
      pos <<- pos + 1L
    }
    if (pos == start) fail(sprintf("unexpected `%s`", chars[[pos]]))
    num <- suppressWarnings(as.numeric(paste(chars[start:(pos - 1L)], collapse = "")))
    if (is.na(num)) fail("invalid number")
    num
  }

  result <- value()
  skip()
  if (pos <= n) fail("unexpected content after the value")
  result
}

# An array of single values of one type becomes an atomic vector.
simplify <- function(x) {
  if (!length(x)) return(x)
  single <- vapply(x, function(v) is.atomic(v) && length(v) == 1L, logical(1))
  if (!all(single)) return(x)
  types <- unique(vapply(x, function(v) class(v)[[1L]], character(1)))
  if (length(types) != 1L) return(x)
  unlist(x, use.names = FALSE)
}

# A list of flat records (named lists of single values) as a data frame.
records_df <- function(records) {
  if (!length(records)) return(data.frame())
  keys <- unique(unlist(lapply(records, names), use.names = FALSE))
  cols <- lapply(keys, function(k) {
    vals <- lapply(records, function(r) {
      v <- r[[k]]
      if (is.null(v) || length(v) != 1L || !is.atomic(v)) NA else v
    })
    unlist(vals, use.names = FALSE)
  })
  names(cols) <- keys
  as.data.frame(cols, stringsAsFactors = FALSE, optional = TRUE)
}
