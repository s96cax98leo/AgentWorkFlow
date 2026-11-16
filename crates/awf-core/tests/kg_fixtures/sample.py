import os
import pkg.mod as pm
from .util import helper, other as o
from ..core import Base


class Service(Base):
    def run(self):
        # @awf:billing/export-invoice#csv
        return self.step() + helper()

    def step(self):
        return pm.compute(os.getcwd())


def main():
    Service().run()
    o()
