"""Microsoft 365 data source syncers."""

from .calendar import CalendarSyncer
from .mail import MailSyncer
from .onedrive import OneDriveSyncer
from .sharepoint import SharePointSyncer

__all__ = ["OneDriveSyncer", "MailSyncer", "CalendarSyncer", "SharePointSyncer"]
