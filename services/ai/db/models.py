import json
import logging
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import Any
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from crypto import decrypt_config
from memory import MemoryMode


logger = logging.getLogger(__name__)


class DoclingQualityPreset(str, Enum):
    FAST = "fast"
    BALANCED = "balanced"
    QUALITY = "quality"


TIMEZONE_ALIASES = {
    "africa/asmera": "Africa/Asmara",
    "africa/timbuktu": "Africa/Bamako",
    "america/argentina/comodrivadavia": "America/Argentina/Catamarca",
    "america/atka": "America/Adak",
    "america/buenos_aires": "America/Argentina/Buenos_Aires",
    "america/catamarca": "America/Argentina/Catamarca",
    "america/coral_harbour": "America/Atikokan",
    "america/cordoba": "America/Argentina/Cordoba",
    "america/ensenada": "America/Tijuana",
    "america/fort_wayne": "America/Indiana/Indianapolis",
    "america/godthab": "America/Nuuk",
    "america/indianapolis": "America/Indiana/Indianapolis",
    "america/jujuy": "America/Argentina/Jujuy",
    "america/knox_in": "America/Indiana/Knox",
    "america/kralendijk": "America/Curacao",
    "america/louisville": "America/Kentucky/Louisville",
    "america/lower_princes": "America/Curacao",
    "america/marigot": "America/Port_of_Spain",
    "america/mendoza": "America/Argentina/Mendoza",
    "america/montreal": "America/Toronto",
    "america/nipigon": "America/Toronto",
    "america/pangnirtung": "America/Iqaluit",
    "america/porto_acre": "America/Rio_Branco",
    "america/rainy_river": "America/Winnipeg",
    "america/rosario": "America/Argentina/Cordoba",
    "america/santa_isabel": "America/Tijuana",
    "america/shiprock": "America/Denver",
    "america/st_barthelemy": "America/Port_of_Spain",
    "america/thunder_bay": "America/Toronto",
    "america/virgin": "America/St_Thomas",
    "america/yellowknife": "America/Edmonton",
    "antarctica/south_pole": "Antarctica/McMurdo",
    "arctic/longyearbyen": "Europe/Oslo",
    "asia/ashkhabad": "Asia/Ashgabat",
    "asia/calcutta": "Asia/Kolkata",
    "asia/choibalsan": "Asia/Ulaanbaatar",
    "asia/chongqing": "Asia/Shanghai",
    "asia/chungking": "Asia/Shanghai",
    "asia/dacca": "Asia/Dhaka",
    "asia/harbin": "Asia/Shanghai",
    "asia/istanbul": "Europe/Istanbul",
    "asia/kashgar": "Asia/Urumqi",
    "asia/katmandu": "Asia/Kathmandu",
    "asia/macao": "Asia/Macau",
    "asia/rangoon": "Asia/Yangon",
    "asia/saigon": "Asia/Ho_Chi_Minh",
    "asia/tel_aviv": "Asia/Jerusalem",
    "asia/thimbu": "Asia/Thimphu",
    "asia/ujung_pandang": "Asia/Makassar",
    "asia/ulan_bator": "Asia/Ulaanbaatar",
    "atlantic/faeroe": "Atlantic/Faroe",
    "atlantic/jan_mayen": "Europe/Oslo",
    "australia/act": "Australia/Sydney",
    "australia/canberra": "Australia/Sydney",
    "australia/currie": "Australia/Hobart",
    "australia/lhi": "Australia/Lord_Howe",
    "australia/north": "Australia/Darwin",
    "australia/nsw": "Australia/Sydney",
    "australia/queensland": "Australia/Brisbane",
    "australia/south": "Australia/Adelaide",
    "australia/tasmania": "Australia/Hobart",
    "australia/victoria": "Australia/Melbourne",
    "australia/west": "Australia/Perth",
    "australia/yancowinna": "Australia/Broken_Hill",
    "brazil/acre": "America/Rio_Branco",
    "brazil/denoronha": "America/Noronha",
    "brazil/east": "America/Sao_Paulo",
    "brazil/west": "America/Manaus",
    "canada/atlantic": "America/Halifax",
    "canada/central": "America/Winnipeg",
    "canada/eastern": "America/Toronto",
    "canada/mountain": "America/Edmonton",
    "canada/newfoundland": "America/St_Johns",
    "canada/pacific": "America/Vancouver",
    "canada/saskatchewan": "America/Regina",
    "canada/yukon": "America/Whitehorse",
    "chile/continental": "America/Santiago",
    "chile/easterisland": "Pacific/Easter",
    "cuba": "America/Havana",
    "egypt": "Africa/Cairo",
    "eire": "Europe/Dublin",
    "etc/gmt+0": "Etc/GMT",
    "etc/gmt-0": "Etc/GMT",
    "etc/gmt0": "Etc/GMT",
    "etc/greenwich": "Etc/GMT",
    "etc/uct": "Etc/UTC",
    "etc/universal": "Etc/UTC",
    "etc/zulu": "Etc/UTC",
    "europe/belfast": "Europe/London",
    "europe/bratislava": "Europe/Prague",
    "europe/busingen": "Europe/Zurich",
    "europe/kiev": "Europe/Kyiv",
    "europe/mariehamn": "Europe/Helsinki",
    "europe/nicosia": "Asia/Nicosia",
    "europe/podgorica": "Europe/Belgrade",
    "europe/san_marino": "Europe/Rome",
    "europe/tiraspol": "Europe/Chisinau",
    "europe/uzhgorod": "Europe/Kyiv",
    "europe/vatican": "Europe/Rome",
    "europe/zaporozhye": "Europe/Kyiv",
    "gb": "Europe/London",
    "gb-eire": "Europe/London",
    "gmt": "Etc/GMT",
    "gmt+0": "Etc/GMT",
    "gmt-0": "Etc/GMT",
    "gmt0": "Etc/GMT",
    "greenwich": "Etc/GMT",
    "hongkong": "Asia/Hong_Kong",
    "iceland": "Atlantic/Reykjavik",
    "iran": "Asia/Tehran",
    "israel": "Asia/Jerusalem",
    "jamaica": "America/Jamaica",
    "japan": "Asia/Tokyo",
    "kwajalein": "Pacific/Kwajalein",
    "libya": "Africa/Tripoli",
    "mexico/bajanorte": "America/Tijuana",
    "mexico/bajasur": "America/Mazatlan",
    "mexico/general": "America/Mexico_City",
    "navajo": "America/Denver",
    "nz": "Pacific/Auckland",
    "nz-chat": "Pacific/Chatham",
    "pacific/enderbury": "Pacific/Kanton",
    "pacific/johnston": "Pacific/Honolulu",
    "pacific/ponape": "Pacific/Pohnpei",
    "pacific/samoa": "Pacific/Pago_Pago",
    "pacific/truk": "Pacific/Chuuk",
    "pacific/yap": "Pacific/Chuuk",
    "poland": "Europe/Warsaw",
    "portugal": "Europe/Lisbon",
    "prc": "Asia/Shanghai",
    "roc": "Asia/Taipei",
    "rok": "Asia/Seoul",
    "singapore": "Asia/Singapore",
    "turkey": "Europe/Istanbul",
    "uct": "Etc/UTC",
    "universal": "Etc/UTC",
    "us/alaska": "America/Anchorage",
    "us/aleutian": "America/Adak",
    "us/arizona": "America/Phoenix",
    "us/central": "America/Chicago",
    "us/east-indiana": "America/Indiana/Indianapolis",
    "us/eastern": "America/New_York",
    "us/hawaii": "Pacific/Honolulu",
    "us/indiana-starke": "America/Indiana/Knox",
    "us/michigan": "America/Detroit",
    "us/mountain": "America/Denver",
    "us/pacific": "America/Los_Angeles",
    "us/samoa": "Pacific/Pago_Pago",
    "utc": "UTC",
    "w-su": "Europe/Moscow",
    "zulu": "Etc/UTC",
}


def _normalize_timezone(timezone: str) -> str | None:
    candidate = timezone.strip()
    if not candidate:
        return None

    canonical = TIMEZONE_ALIASES.get(candidate.lower(), candidate)
    try:
        ZoneInfo(canonical)
    except ZoneInfoNotFoundError:
        logger.warning("Ignoring invalid user timezone configuration: %s", timezone)
        return None
    return canonical


@dataclass(frozen=True)
class GlobalConfiguration:
    docling_enabled: bool = False
    docling_quality_preset: DoclingQualityPreset = DoclingQualityPreset.BALANCED
    memory_mode_default: MemoryMode = MemoryMode.OFF
    memory_llm_id: str | None = None

    @classmethod
    def from_rows(cls, rows: list[dict[str, Any]]) -> "GlobalConfiguration":
        values = {row["key"]: row.get("value") for row in rows}

        docling_enabled = _read_configuration_bool(values.get("docling_enabled"))
        raw_preset = _read_configuration_string(
            values.get("docling_quality_preset"), "preset"
        )
        raw_memory_mode = _read_configuration_string(
            values.get("memory_mode_default"), "mode"
        )
        memory_llm_id = _read_configuration_string(values.get("memory_llm_id"))

        try:
            docling_quality_preset = (
                DoclingQualityPreset(raw_preset)
                if raw_preset
                else DoclingQualityPreset.BALANCED
            )
        except ValueError as exc:
            raise ValueError(
                f"Invalid docling_quality_preset configuration: {raw_preset}"
            ) from exc

        memory_mode_default = MemoryMode.parse(raw_memory_mode)
        if raw_memory_mode and memory_mode_default is None:
            raise ValueError(
                f"Invalid memory_mode_default configuration: {raw_memory_mode}"
            )

        return cls(
            docling_enabled=docling_enabled if docling_enabled is not None else False,
            docling_quality_preset=docling_quality_preset,
            memory_mode_default=memory_mode_default or MemoryMode.OFF,
            memory_llm_id=memory_llm_id,
        )


def _decode_configuration_value(raw: Any) -> Any:
    if not isinstance(raw, str):
        return raw
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        return raw


def _read_configuration_string(raw: Any, *alternate_keys: str) -> str | None:
    decoded = _decode_configuration_value(raw)
    if isinstance(decoded, str):
        return decoded
    if isinstance(decoded, dict):
        for key in ("value", *alternate_keys):
            value = decoded.get(key)
            if isinstance(value, str):
                return value
    return None


def _read_configuration_bool(raw: Any) -> bool | None:
    decoded = _decode_configuration_value(raw)
    if isinstance(decoded, bool):
        return decoded
    if isinstance(decoded, dict):
        value = decoded.get("enabled")
        if isinstance(value, bool):
            return value
    return None


@dataclass(frozen=True)
class UserConfiguration:
    memory_mode: MemoryMode | None = None
    timezone: str | None = None

    @classmethod
    def from_rows(cls, rows: list[dict[str, Any]]) -> "UserConfiguration | None":
        if not rows:
            return None

        values = {row["key"]: row.get("value") for row in rows}
        timezone = (
            _read_configuration_string(values["timezone"], "timezone")
            if "timezone" in values
            else None
        )
        if timezone:
            timezone = _normalize_timezone(timezone)

        raw_memory_mode = (
            _read_configuration_string(values["memory_mode"], "mode")
            if "memory_mode" in values
            else None
        )
        memory_mode = MemoryMode.parse(raw_memory_mode)
        if raw_memory_mode and memory_mode is None:
            raise ValueError(f"Invalid user memory_mode configuration: {raw_memory_mode}")

        return cls(memory_mode=memory_mode, timezone=timezone)


@dataclass
class User:
    id: str
    email: str
    full_name: str | None
    role: str
    is_active: bool
    created_at: datetime
    updated_at: datetime
    configuration: UserConfiguration | None = None

    @property
    def timezone(self) -> str | None:
        return self.configuration.timezone if self.configuration else None

    @classmethod
    def from_row(cls, row: dict) -> "User":
        return cls(
            id=row["id"],
            email=row["email"],
            full_name=row.get("full_name"),
            role=row["role"],
            is_active=row["is_active"],
            created_at=row["created_at"],
            updated_at=row["updated_at"],
            configuration=row.get("configuration"),
        )


class ChatRole(str, Enum):
    USER = "user"
    ASSISTANT = "assistant"
    SYSTEM = "system"


@dataclass
class Chat:
    id: str
    user_id: str
    title: str | None
    model_id: str | None
    created_at: datetime
    updated_at: datetime
    agent_id: str | None = None

    @classmethod
    def from_row(cls, row: dict) -> "Chat":
        """Create Chat from database row"""
        model_id = row.get("model_id")
        if model_id:
            model_id = model_id.strip()
        return cls(
            id=row["id"],
            user_id=row["user_id"],
            title=row.get("title"),
            model_id=model_id,
            created_at=row["created_at"],
            updated_at=row["updated_at"],
            agent_id=row.get("agent_id"),
        )

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization"""
        return {
            "id": self.id,
            "user_id": self.user_id,
            "title": self.title,
            "model_id": self.model_id,
            "agent_id": self.agent_id,
            "created_at": self.created_at.isoformat(),
            "updated_at": self.updated_at.isoformat(),
        }


@dataclass
class ModelRecord:
    id: str
    model_provider_id: str
    model_id: str
    display_name: str
    is_default: bool
    is_secondary: bool
    is_deleted: bool
    provider_type: str
    config: dict
    created_at: datetime
    updated_at: datetime

    @classmethod
    def from_row(cls, row: dict) -> "ModelRecord":
        config = row["config"]
        if isinstance(config, str):
            config = json.loads(config)
        config = decrypt_config(config)
        return cls(
            id=row["id"].strip(),
            model_provider_id=row["model_provider_id"].strip(),
            model_id=row["model_id"],
            display_name=row["display_name"],
            is_default=row["is_default"],
            is_secondary=row["is_secondary"],
            is_deleted=row["is_deleted"],
            provider_type=row["provider_type"],
            config=config,
            created_at=row["created_at"],
            updated_at=row["updated_at"],
        )


@dataclass
class Source:
    id: str
    name: str
    source_type: str
    is_active: bool
    is_deleted: bool

    @classmethod
    def from_row(cls, row: dict) -> "Source":
        return cls(
            id=row["id"],
            name=row["name"],
            source_type=row["source_type"],
            is_active=row["is_active"],
            is_deleted=row["is_deleted"],
        )


@dataclass
class ChatMessage:
    id: str
    chat_id: str
    message_seq_num: int
    message: dict[str, Any]  # Full JSONB message content
    created_at: datetime
    parent_id: str | None = None

    @classmethod
    def from_row(cls, row: dict) -> "ChatMessage":
        """Create ChatMessage from database row"""
        if isinstance(row["message"], str):
            row["message"] = json.loads(row["message"])
        return cls(
            id=row["id"],
            chat_id=row["chat_id"],
            message_seq_num=row["message_seq_num"],
            message=row["message"],
            created_at=row["created_at"],
            parent_id=row.get("parent_id"),
        )

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization"""
        return {
            "id": self.id,
            "chat_id": self.chat_id,
            "message_seq_num": self.message_seq_num,
            "message": self.message,
            "parent_id": self.parent_id,
            "created_at": self.created_at.isoformat(),
        }
