from pydantic import BaseModel, Field


class GitHubSourceConfig(BaseModel):
    api_url: str | None = None
    include_discussions: bool = True
    include_forks: bool = False
    repos: list[str] = Field(default_factory=list)
    orgs: list[str] = Field(default_factory=list)
    users: list[str] = Field(default_factory=list)
    read_only: bool = False


class GitHubCredentials(BaseModel):
    token: str
