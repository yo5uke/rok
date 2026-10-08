#' @details
#' rok manages the packages and the R version of a project. Packages are declared in
#' `rok.toml`, resolved from dated snapshots of Posit Package Manager, and recorded in
#' `rok.lock`. The work is done by the rok binary, which [setup()] installs; the functions of
#' this package call it.
#'
#' # Package options
#'
#' * `rok.binary`: the path of the rok binary to use (see [rok_binary()]).
#' * `rok.yes`: answer yes to every question, for non-interactive sessions.
#' * `rok.release_url`: where [setup()] downloads the binary from.
#'
#' @keywords internal
"_PACKAGE"
