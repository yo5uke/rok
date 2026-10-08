# Development check, not part of R CMD check (the package depends on base R only, and Matrix
# is a recommended package): with Matrix attached before rok, update() still reaches Matrix's
# S4 generic and its methods. Run with an installed rok package:
#   Rscript rpkg/dev/masking-matrix.R
library(rok)
same_fit <- function(a, b) {
  identical(stats::coef(a), stats::coef(b)) && identical(a$call, b$call)
}
fit <- lm(dist ~ speed, data = cars)

# Matrix (a recommended package) defines an S4 generic update(). With rok attached after it,
# calls still reach Matrix's generic, and its S4 methods still dispatch.
if (requireNamespace("Matrix", quietly = TRUE)) {
  suppressPackageStartupMessages(library(Matrix))
  if (!identical(environmentName(environment(update)), "rok")) {
    detach("package:rok")
    suppressPackageStartupMessages(library(rok))
  }
  stopifnot(identical(environmentName(environment(update)), "rok"))
  A <- Matrix::Matrix(c(4, 1, 1, 3), 2, 2, sparse = TRUE)
  L <- Matrix::Cholesky(A)
  L2 <- update(L, A * 2)
  stopifnot(methods::is(L2, "CHMfactor"))
  stopifnot(same_fit(update(fit, . ~ 1), stats::update(fit, . ~ 1)))
}
cat("masking-matrix: OK\n")
