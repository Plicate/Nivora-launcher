class LauncherError(Exception):
    """A user-facing launcher error."""


class AuthenticationError(LauncherError):
    """Microsoft or account authentication failed."""


class IntegrityError(LauncherError):
    """A downloaded file failed its expected integrity check."""
