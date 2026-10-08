# The JSON parser (base R only, so no testthat).
parse_json <- rok:::parse_json
records_df <- rok:::records_df

stopifnot(
  identical(parse_json("true"), TRUE),
  identical(parse_json(" null "), NULL),
  identical(parse_json("-1.5e2"), -150),
  identical(parse_json("\"a\\\"b\\\\c\\n\\u00e9\\ud83d\\ude00\""), "a\"b\\c\né\U0001F600"),
  identical(parse_json("[1, 2, 3]"), c(1, 2, 3)),
  identical(parse_json("[\"x\", \"y\"]"), c("x", "y")),
  identical(parse_json("[1, \"x\"]"), list(1, "x")),
  identical(parse_json("[]"), list()),
  identical(parse_json("{}"), structure(list(), names = character()))
)

obj <- parse_json('{"command": "status", "ok": false, "r": null, "problems": [
  {"level": "warning", "code": "lock-outdated", "details": ["a", "b"], "fix": null}],
  "packages": [{"name": "fixest", "version": "0.12.1", "declared": true},
               {"name": "Rcpp", "version": "1.1.2", "declared": false}]}')
stopifnot(
  identical(obj$command, "status"),
  identical(obj$ok, FALSE),
  "r" %in% names(obj) && is.null(obj$r),
  identical(obj$problems[[1]]$details, c("a", "b")),
  identical(obj$packages[[2]]$name, "Rcpp")
)
df <- records_df(obj$packages)
stopifnot(
  identical(df$name, c("fixest", "Rcpp")),
  identical(df$declared, c(TRUE, FALSE)),
  identical(nrow(records_df(list())), 0L)
)

for (bad in c("", "[1,", "{\"a\" 1}", "tru", "\"open", "[1] x")) {
  stopifnot(inherits(tryCatch(parse_json(bad), error = identity), "error"))
}
