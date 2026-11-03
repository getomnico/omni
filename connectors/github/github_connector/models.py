from urllib.parse import urlsplit

from pydantic import BaseModel, Field, field_validator, model_validator


class GitHubSourceConfig(BaseModel):
    api_url: str | None = None
    api_version: str | None = Field(default=None, pattern=r"^\d{4}-\d{2}-\d{2}$")
    include_discussions: bool = True
    include_forks: bool = False
    repos: list[str] = Field(default_factory=list)
    orgs: list[str] = Field(default_factory=list)
    users: list[str] = Field(default_factory=list)
    read_only: bool = False

    @field_validator("api_url")
    @classmethod
    def normalise_api_url(cls, value: str | None) -> str | None:
        if value is None:
            return None
        url = value.strip().rstrip("/")
        if not url:
            return None
        parts = urlsplit(url)
        if parts.scheme not in ("http", "https") or not parts.hostname:
            raise ValueError(f"api_url must be an absolute http(s) URL, got {value!r}")
        if parts.hostname == "api.github.com":
            return None
        # GHE.com (data residency) serves REST at the host root, unlike GHES.
        return url if parts.path or parts.hostname.endswith(".ghe.com") else f"{url}/api/v3"

    @property
    def host(self) -> str | None:
        if self.api_url is None:
            return None
        parts = urlsplit(self.api_url)
        return f"{parts.scheme}://{parts.netloc}"


class GitHubCredentials(BaseModel):
    token: str | None = None
    access_token: str | None = None

    @model_validator(mode="after")
    def require_token(self) -> "GitHubCredentials":
        if not self.token and not self.access_token:
            raise ValueError("Missing 'token' or 'access_token' in credentials")
        return self

    @property
    def effective_token(self) -> str:
        token = self.token or self.access_token
        if token is None:
            raise ValueError("Missing 'token' or 'access_token' in credentials")
        return token
