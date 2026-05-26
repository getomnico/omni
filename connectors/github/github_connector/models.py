from pydantic import BaseModel, Field, model_validator


class GitHubSourceConfig(BaseModel):
    api_url: str | None = None
    include_discussions: bool = True
    include_forks: bool = False
    repos: list[str] = Field(default_factory=list)
    orgs: list[str] = Field(default_factory=list)
    users: list[str] = Field(default_factory=list)
    read_only: bool = False


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
