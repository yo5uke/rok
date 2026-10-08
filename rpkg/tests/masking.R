# update() and remove() mask stats::update() and base::remove() when rok is attached. Calls
# meant for those must behave exactly as without rok; calls meant for rok must reach rok.
library(rok)
stopifnot(identical(environmentName(environment(update)), "rok"))

# Record what would be sent to the binary instead of running it.
sent <- NULL
utils::assignInNamespace("rok_call", function(args) {
  sent <<- args
  invisible(args)
}, ns = "rok")
reset <- function() sent <<- NULL

same_fit <- function(a, b) {
  identical(stats::coef(a), stats::coef(b)) && identical(a$call, b$call)
}

# ---- update(): models and formulas go to stats::update() ----

fit <- lm(dist ~ speed, data = cars)
stopifnot(same_fit(update(fit, . ~ . + I(speed^2)), stats::update(fit, . ~ . + I(speed^2))))

# The refit is evaluated in the caller's frame, so local data is found.
local_refit <- function() {
  d <- cars[cars$speed > 5, ]
  m <- lm(dist ~ speed, data = d)
  update(m, . ~ 1)
}
stopifnot(identical(nrow(stats::model.frame(local_refit())), sum(cars$speed > 5)))

# Extra arguments stay as written in the stored call (not replaced by their values).
d2 <- cars[1:20, ]
u <- update(fit, data = d2)
stopifnot(identical(u$call$data, quote(d2)), identical(nrow(stats::model.frame(u)), 20L))
stopifnot(identical(
  update(fit, . ~ . - 1, evaluate = FALSE),
  stats::update(fit, . ~ . - 1, evaluate = FALSE)
))

# Formulas, glm, and functions passed as values.
stopifnot(identical(update(y ~ x, ~ . + z), stats::update(y ~ x, ~ . + z)))
g <- glm(am ~ wt, data = mtcars, family = binomial)
stopifnot(same_fit(update(g, . ~ . + hp), stats::update(g, . ~ . + hp)))
fits <- lapply(list(fit, fit), update, . ~ 1)
stopifnot(same_fit(fits[[2]], stats::update(fit, . ~ 1)))
forward <- function(...) update(...)
stopifnot(same_fit(forward(fit, . ~ 1), stats::update(fit, . ~ 1)))
stopifnot(same_fit(do.call("update", list(fit, . ~ 1)), stats::update(fit, . ~ 1)))
if (getRversion() >= "4.1.0") {
  piped <- eval(parse(text = "fit |> update(. ~ 1)"))
  stopifnot(same_fit(piped, stats::update(fit, . ~ 1)))
}

# A first argument written as a call is evaluated once, not twice.
calls <- 0
make_fit <- function() {
  calls <<- calls + 1
  lm(dist ~ speed, data = cars)
}
invisible(update(make_fit(), . ~ 1))
stopifnot(identical(calls, 1))

# rok::update() with a model also goes to stats::update().
stopifnot(same_fit(rok::update(fit, . ~ 1), stats::update(fit, . ~ 1)))

# Errors are those of stats::update().
err <- tryCatch(update(1, . ~ .), error = conditionMessage)
stopifnot(identical(err, tryCatch(stats::update(1, . ~ .), error = conditionMessage)))

# ---- update(): package names go to rok ----

reset(); update()
stopifnot(identical(sent, c("update", "--project", ".")))
reset(); update("sf", "terra")
stopifnot(identical(sent, c("update", "sf", "terra", "--project", ".")))
pkgs <- c("sf", "terra")
reset(); update(pkgs, dry_run = TRUE)
stopifnot(identical(sent, c("update", "sf", "terra", "--dry-run", "--project", ".")))
reset(); update(to = "2026-01-31")
stopifnot(identical(sent, c("update", "--to", "2026-01-31", "--project", ".")))
stopifnot(inherits(tryCatch(update(character()), error = identity), "error"))

# ---- remove(): unquoted names and base arguments go to base::remove() ----

local_remove <- function() {
  x <- 1
  y <- 2
  remove(x)
  c(exists("x", inherits = FALSE), exists("y", inherits = FALSE))
}
x <- "global"
stopifnot(identical(local_remove(), c(FALSE, TRUE)), identical(x, "global"))

local_list <- function() {
  a <- 1
  b <- 2
  remove(list = c("a", "b"))
  exists("a", inherits = FALSE) || exists("b", inherits = FALSE)
}
stopifnot(!local_list())

e <- new.env()
assign("z", 1, envir = e)
remove(z, envir = e)
stopifnot(!exists("z", envir = e, inherits = FALSE))

tmp <- 1
remove(tmp)
stopifnot(!exists("tmp", envir = globalenv(), inherits = FALSE))

# rm() is base's and untouched.
stopifnot(identical(rm, base::rm))

# A wrapper that forwards `...` behaves as it would with base::remove().
with_rok <- function(...) remove(...)
with_base <- function(...) base::remove(...)
w1 <- tryCatch(with_rok(nothing_here), warning = conditionMessage)
w2 <- tryCatch(with_base(nothing_here), warning = conditionMessage)
stopifnot(identical(w1, w2))

# ---- remove(): package names go to rok ----

reset(); remove("sf")
stopifnot(identical(sent, c("remove", "sf", "--project", ".")))
reset(); remove(c("sf", "terra"))
stopifnot(identical(sent, c("remove", "sf", "terra", "--project", ".")))
pkgs <- c("sf", "terra")
reset(); rok::remove(pkgs)
stopifnot(identical(sent, c("remove", "sf", "terra", "--project", ".")), exists("pkgs"))
